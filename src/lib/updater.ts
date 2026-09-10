import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

/**
 * Every fix this build has shipped so far meant telling Mona to manually
 * download a new .dmg, delete the old app, and drag the new one in —
 * repeated often enough that she pushed back on it directly. This wraps
 * Tauri's updater plugin, which checks the endpoint configured in
 * tauri.conf.json (the GitHub release's `latest.json`, signed at build
 * time — see .github/workflows/build-macos.yml) so App.tsx can offer a
 * one-click "install and restart" instead.
 */
export const checkForUpdate = () => check();

/**
 * REAL BUG found 2026-09-10: the download itself works (a real ~65MB
 * universal build), but this used to call `downloadAndInstall()` with no
 * progress callback and no timeout — so the button just said "جاري
 * التحديث…" the entire time, with nothing distinguishing "still genuinely
 * downloading" from "silently hung forever." Mona reported it as stuck
 * and, told to work around it with a manual .dmg download instead, pushed
 * back explicitly: she wants the actual updater fixed, not routed around.
 * `downloadAndInstall`'s own API already supports both of what was
 * missing — this just wires them through:
 *   - `onProgress` reports real percent-complete (from the Started event's
 *     contentLength and each Progress event's chunkLength) so the UI can
 *     show a real number instead of a static string with no signal.
 *   - `timeout` (per Tauri's own DownloadOptions — a bound on the whole
 *     request, not a per-chunk one) means a connection that genuinely
 *     stalls now fails with a real, visible error after 5 minutes instead
 *     of hanging indefinitely with the button stuck disabled and no way
 *     to recover short of force-quitting the app. 5 minutes is generous
 *     for ~65MB even on a slow connection, while still being a real bound
 *     instead of the previous "forever."
 */
export const installUpdateAndRestart = async (
  update: Update,
  onProgress?: (percent: number | null) => void,
) => {
  let contentLength: number | undefined;
  let downloaded = 0;
  await update.downloadAndInstall(
    (event) => {
      if (event.event === "Started") {
        contentLength = event.data.contentLength;
        downloaded = 0;
        onProgress?.(contentLength ? 0 : null);
      } else if (event.event === "Progress") {
        downloaded += event.data.chunkLength;
        onProgress?.(
          contentLength ? Math.min(99, Math.round((downloaded / contentLength) * 100)) : null,
        );
      } else if (event.event === "Finished") {
        onProgress?.(100);
      }
    },
    { timeout: 5 * 60 * 1000 },
  );
  await relaunch();
};
