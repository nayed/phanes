/**
 * Changes made outside Phanes (docs/PHANES.md, "Changes made outside
 * Phanes"): another editor saving an open file, `git checkout`, files added
 * or removed in GNOME Files…
 *
 * Rust watches the folders Phanes shows, without their subfolders: the
 * workspace root, its expanded folders and the folders of open files. That
 * set is kept in step with the tree and the tabs here. Each burst of changes
 * then updates the open files (tabs.ts decides between reloading and the
 * banner) and the tree.
 */
import { onFilesChanged, watchDirs } from "../lib/fs";
import { basename, dirname } from "../lib/path";
import { followDiskChange, useTabs } from "./tabs";
import { refreshTree, useWorkspace } from "./workspace";

/** The folders to watch now. */
function wantedDirs(): string[] {
  const { root, expanded } = useWorkspace.getState();
  const dirs = new Set<string>(root ? [root, ...expanded] : []);
  for (const tab of useTabs.getState().tabs) if (tab.path) dirs.add(dirname(tab.path));
  return [...dirs].sort();
}

let watched = "";
let syncTimer: number | undefined;

/** Tell Rust the folders to watch, once things settle and only when they changed. */
function scheduleSync(): void {
  window.clearTimeout(syncTimer);
  syncTimer = window.setTimeout(() => {
    const dirs = wantedDirs();
    const key = dirs.join("\n");
    if (key === watched) return;
    watched = key;
    watchDirs(dirs).catch((err) => console.warn("Phanes can't watch for outside changes:", err));
  }, 100);
}

async function onChanges(paths: string[]): Promise<void> {
  const changed = new Set(paths);
  const tabs = useTabs.getState().tabs.filter((t) => t.path && changed.has(t.path));
  await Promise.all(tabs.map((t) => followDiskChange(t.id)));

  // The tree lists the root and the expanded folders; hidden names never show.
  const { root, expanded } = useWorkspace.getState();
  if (!root) return;
  const listed = new Set([root, ...expanded]);
  if (paths.some((p) => listed.has(dirname(p)) && !basename(p).startsWith("."))) await refreshTree();
}

/** Start following outside changes. Once, at startup. */
export function installDiskWatch(): void {
  useWorkspace.subscribe((s, prev) => {
    if (s.root !== prev.root || s.expanded !== prev.expanded) scheduleSync();
  });
  useTabs.subscribe((s, prev) => {
    if (s.tabs !== prev.tabs) scheduleSync();
  });
  scheduleSync();
  void onFilesChanged((paths) => void onChanges(paths));
}
