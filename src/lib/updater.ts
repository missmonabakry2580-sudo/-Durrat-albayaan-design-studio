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

export const installUpdateAndRestart = async (update: Update) => {
  await update.downloadAndInstall();
  await relaunch();
};
