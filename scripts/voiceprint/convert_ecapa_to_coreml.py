#!/usr/bin/env python3
"""Converts SpeechBrain's pretrained ECAPA-TDNN speaker-embedding model to a
CoreML .mlpackage for on-device speaker verification (see
macos/transcriber/VoicePrint.swift and docs/ARCHITECTURE.md's "Voice
biometrics" section).

Run once (or whenever the pinned source model changes), not at app runtime:
    python3 scripts/voiceprint/convert_ecapa_to_coreml.py

Fixed 3-second/16kHz input window: both enrollment and verification buffer
audio to exactly SAMPLE_RATE * WINDOW_SECONDS samples on the Swift side
before calling the model, so the graph needs no dynamic-shape support.
"""

import sys

import coremltools as ct
import torch
from speechbrain.inference.speaker import EncoderClassifier


def _traceable_length_to_mask(length, max_len=None, dtype=None, device=None):
    """Drop-in replacement for speechbrain.dataio.dataio.length_to_mask.

    The original calls `len(length)` and `torch.as_tensor(mask, ...)` on a
    tensor — both trace as an `aten::Int` scalar-conversion op that
    coremltools' PyTorch frontend cannot lower ("only 0-dimensional arrays
    can be converted to Python scalars"). `length.shape[0]` and `.to(...)`
    are numerically identical for tensor inputs (the only inputs any call
    site in this model ever passes) and trace to static ops instead.
    """
    if max_len is None:
        max_len = length.max().long().item()
    mask = torch.arange(
        max_len, device=length.device, dtype=length.dtype
    ).expand(length.shape[0], max_len) < length.unsqueeze(1)
    return mask.to(dtype=dtype or length.dtype, device=device or length.device)


def _patch_length_to_mask_everywhere() -> None:
    """speechbrain's TDNN/SE/pooling blocks each did `from ... import
    length_to_mask`, binding their own local reference at import time —
    patching the defining module's attribute alone wouldn't reach those.
    Sweep every already-imported speechbrain submodule instead."""
    patched = []
    for name, module in list(sys.modules.items()):
        if not name.startswith("speechbrain"):
            continue
        try:
            has_it = getattr(module, "length_to_mask", None) is not None
        except Exception:
            # speechbrain lazy-loads optional integrations (e.g. k2_fsa)
            # behind __getattr__; touching those with a plain getattr
            # triggers a real import of a package we don't have and don't
            # need. Not our model's dependency graph — skip it.
            continue
        if has_it:
            module.length_to_mask = _traceable_length_to_mask
            patched.append(name)
    print(f"Patched length_to_mask in: {patched}")


SOURCE_MODEL = "speechbrain/spkrec-ecapa-voxceleb"
SAMPLE_RATE = 16000
WINDOW_SECONDS = 3
NUM_SAMPLES = SAMPLE_RATE * WINDOW_SECONDS
OUTPUT_PATH = "macos/transcriber/Resources/ECAPA_TDNN.mlpackage"


class SpeakerEmbedder(torch.nn.Module):
    """wav[1, NUM_SAMPLES] -> L2-normalized speaker embedding[1, 192].

    Mirrors EncoderClassifier.encode_batch's feature/norm/embedding chain,
    but inlines mean_var_norm's math instead of calling it: for a fixed
    full-length window, InputNormalization("sentence", std_norm=False)
    reduces to a plain per-utterance mean subtraction over the time axis
    (confirmed against its source — the masked-mean/std bookkeeping it
    otherwise does is a no-op once every frame is real, not padding).
    Calling it directly traces a `make_padding_mask`/`len(lengths)` path
    that isn't representable in CoreML's static graph
    ("only 0-dimensional arrays can be converted to Python scalars").
    """

    def __init__(self, mods: torch.nn.ModuleDict):
        super().__init__()
        self.compute_features = mods["compute_features"]
        self.embedding_model = mods["embedding_model"]

    def forward(self, wav: torch.Tensor) -> torch.Tensor:
        # Fixed batch size of 1 (see module docstring) — a literal avoids
        # tracing `wav.shape[0]` into a dynamic aten::size/aten::Int chain
        # that coremltools' frontend can't lower to a constant.
        lens = torch.ones(1)
        feats = self.compute_features(wav)
        feats = feats - feats.mean(dim=1, keepdim=True)
        embedding = self.embedding_model(feats, lens).squeeze(1)
        return torch.nn.functional.normalize(embedding, p=2, dim=1)


def main() -> None:
    print(f"Downloading/loading {SOURCE_MODEL} ...")
    classifier = EncoderClassifier.from_hparams(source=SOURCE_MODEL)
    _patch_length_to_mask_everywhere()
    embedder = SpeakerEmbedder(classifier.mods).eval()

    example = torch.randn(1, NUM_SAMPLES)
    with torch.no_grad():
        inlined_output = embedder(example)
        real_output = torch.nn.functional.normalize(
            classifier.encode_batch(example).squeeze(1), p=2, dim=1
        )
    max_diff = (inlined_output - real_output).abs().max().item()
    print(f"Inlined vs. real mean_var_norm max abs diff: {max_diff:.2e}")
    if max_diff > 1e-5:
        sys.exit(
            "Inlined normalization diverges from speechbrain's own "
            "mean_var_norm — refusing to ship a silently-wrong model."
        )

    traced = torch.jit.trace(embedder, example)

    mlmodel = ct.convert(
        traced,
        inputs=[ct.TensorType(name="waveform", shape=(1, NUM_SAMPLES))],
        outputs=[ct.TensorType(name="embedding")],
        minimum_deployment_target=ct.target.macOS13,
        source="pytorch",
        convert_to="mlprogram",
    )
    mlmodel.short_description = (
        "ECAPA-TDNN speaker embedding (speechbrain/spkrec-ecapa-voxceleb), "
        f"fixed {WINDOW_SECONDS}s/{SAMPLE_RATE}Hz mono input, L2-normalized "
        "192-dim output for cosine-similarity speaker verification."
    )
    mlmodel.save(OUTPUT_PATH)
    print(f"Saved {OUTPUT_PATH}")


if __name__ == "__main__":
    sys.exit(main())
