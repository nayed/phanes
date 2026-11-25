import { getCommand, runCommand } from "../commands/registry";
import { activeTab, useTabs } from "../state/tabs";

/**
 * A GNOME-style banner (like GNOME Text Editor's) above the editor and
 * preview when the active tab's file changed on disk while it has unsaved
 * changes, or the file is gone. Unchanged tabs reload by themselves and never
 * show it (tabs.ts, followDiskChange).
 */
export function DiskBanner() {
  const disk = useTabs((s) => activeTab(s)?.disk);
  if (!disk) return null;
  return (
    <div className="disk-banner" role="status">
      <span className="disk-banner-title">
        {disk === "changed" ? "File Has Changed on Disk" : "File Was Moved or Deleted"}
      </span>
      {disk === "changed" && (
        <button type="button" className="pill-button" onClick={() => void runCommand("file.reload")}>
          {getCommand("file.reload").label}
        </button>
      )}
    </div>
  );
}
