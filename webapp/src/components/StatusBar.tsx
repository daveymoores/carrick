import type { GraphStats } from "@/types/graph";

interface StatusBarProps {
  stats: GraphStats;
  generatedAt?: string;
}

export function StatusBar({ stats, generatedAt }: StatusBarProps) {
  return (
    <div className="flex items-center gap-4 px-4 py-2 bg-zinc-900 border-t border-zinc-800 text-xs text-zinc-400">
      <span>
        {stats.totalServices} service{stats.totalServices !== 1 ? "s" : ""}
      </span>
      <span className="text-zinc-700">|</span>
      <span>{stats.totalEndpoints} endpoints</span>
      <span className="text-zinc-700">|</span>
      <span>{stats.totalConnections} connections</span>
      {stats.typeMismatches > 0 && (
        <>
          <span className="text-zinc-700">|</span>
          <span className="text-red-400">
            {stats.typeMismatches} mismatch{stats.typeMismatches !== 1 ? "es" : ""}
          </span>
        </>
      )}
      <span className="ml-auto text-zinc-600">
        {generatedAt && new Date(generatedAt).toLocaleString()}
      </span>
    </div>
  );
}
