import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Share2, Check, Link, Loader2 } from "lucide-react";
import { createSnapshot } from "@/lib/api";

interface SharePopoverProps {
  org: string;
}

export function SharePopover({ org }: SharePopoverProps) {
  const [open, setOpen] = useState(false);
  const [loading, setLoading] = useState(false);
  const [snapshotUrl, setSnapshotUrl] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  async function handleCreateSnapshot() {
    setLoading(true);
    try {
      const result = await createSnapshot(org);
      const url = `${window.location.origin}/snapshot/${result.snapshotId}`;
      setSnapshotUrl(url);
    } catch {
      // Silently fail — user can retry
    } finally {
      setLoading(false);
    }
  }

  async function handleCopy() {
    if (!snapshotUrl) return;
    await navigator.clipboard.writeText(snapshotUrl);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  }

  return (
    <div className="relative">
      <Button
        variant="outline"
        size="sm"
        onClick={() => {
          setOpen(!open);
          if (!open && !snapshotUrl) handleCreateSnapshot();
        }}
      >
        <Share2 className="h-4 w-4 mr-1" />
        Share
      </Button>

      {open && (
        <div className="absolute right-0 top-10 w-72 bg-zinc-900 border border-zinc-800 rounded-lg shadow-xl p-3 z-50">
          {loading ? (
            <div className="flex items-center gap-2 text-sm text-zinc-400">
              <Loader2 className="h-4 w-4 animate-spin" />
              Creating snapshot...
            </div>
          ) : snapshotUrl ? (
            <div className="space-y-2">
              <p className="text-xs text-zinc-400">Shareable snapshot link:</p>
              <div className="flex items-center gap-2">
                <input
                  readOnly
                  value={snapshotUrl}
                  className="flex-1 bg-zinc-800 border border-zinc-700 rounded px-2 py-1 text-xs text-zinc-300 font-mono"
                />
                <Button variant="ghost" size="icon" onClick={handleCopy}>
                  {copied ? (
                    <Check className="h-4 w-4 text-green-500" />
                  ) : (
                    <Link className="h-4 w-4" />
                  )}
                </Button>
              </div>
            </div>
          ) : (
            <Button variant="outline" size="sm" onClick={handleCreateSnapshot}>
              Generate link
            </Button>
          )}
        </div>
      )}
    </div>
  );
}
