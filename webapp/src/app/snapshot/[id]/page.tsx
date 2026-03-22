"use client";

import { use, useState, useCallback } from "react";
import { useQuery } from "@tanstack/react-query";
import { fetchSnapshot } from "@/lib/api";
import { GraphCanvas } from "@/components/graph/GraphCanvas";
import { ServiceSheet } from "@/components/detail/ServiceSheet";
import { ConnectionSheet } from "@/components/detail/ConnectionSheet";
import { StatusBar } from "@/components/StatusBar";
import { Button } from "@/components/ui/button";
import { Loader2, Check, Link } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import type { Service, Connection } from "@/types/graph";

export default function SnapshotPage({
  params,
}: {
  params: Promise<{ id: string }>;
}) {
  const { id } = use(params);
  const [selectedService, setSelectedService] = useState<Service | null>(null);
  const [selectedConnection, setSelectedConnection] =
    useState<Connection | null>(null);
  const [copied, setCopied] = useState(false);

  // Snapshot needs an org to fetch — we pass a placeholder since the
  // snapshot endpoint stores the full graph data. The API route extracts org
  // from the stored snapshot.
  // For now, use a query param or default. The Lambda stores org inside the snapshot.
  const { data, isLoading, error } = useQuery({
    queryKey: ["snapshot", id],
    queryFn: async () => {
      // Try to fetch the snapshot — the API needs the org in the path.
      // We'll try a well-known pattern: the snapshot ID is globally unique
      // so we use a wildcard org approach.
      const res = await fetch(
        `${process.env.NEXT_PUBLIC_API_URL || ""}/graph/_/snapshot/${id}`,
      );
      if (!res.ok) throw new Error(`Snapshot not found: ${res.status}`);
      return res.json();
    },
  });

  const handleSelectService = useCallback((service: Service) => {
    setSelectedService(service);
    setSelectedConnection(null);
  }, []);

  const handleSelectConnection = useCallback((connection: Connection) => {
    setSelectedConnection(connection);
    setSelectedService(null);
  }, []);

  const handleClearSelection = useCallback(() => {
    setSelectedService(null);
    setSelectedConnection(null);
  }, []);

  async function handleCopyLink() {
    await navigator.clipboard.writeText(window.location.href);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  }

  if (isLoading) {
    return (
      <div className="min-h-screen flex items-center justify-center">
        <Loader2 className="h-8 w-8 animate-spin text-zinc-400" />
      </div>
    );
  }

  if (error || !data) {
    return (
      <div className="min-h-screen flex flex-col items-center justify-center gap-4 text-zinc-400">
        <p>Snapshot not found</p>
        <a href="/" className="text-zinc-500 hover:text-zinc-300 underline text-sm">
          Go home
        </a>
      </div>
    );
  }

  return (
    <div className="h-screen flex flex-col">
      {/* Header */}
      <header className="flex items-center justify-between px-4 py-3 bg-zinc-900 border-b border-zinc-800">
        <div className="flex items-center gap-3">
          <h1 className="text-lg font-semibold text-zinc-50">Carrick Graph</h1>
          <Badge variant="secondary">Snapshot</Badge>
          {data.org && (
            <span className="text-sm text-zinc-400">{data.org}</span>
          )}
        </div>
        <Button variant="outline" size="sm" onClick={handleCopyLink}>
          {copied ? (
            <Check className="h-4 w-4 mr-1 text-green-500" />
          ) : (
            <Link className="h-4 w-4 mr-1" />
          )}
          Copy link
        </Button>
      </header>

      {/* Main content */}
      <div className="flex flex-1 overflow-hidden">
        <div className="flex-1 relative">
          <GraphCanvas
            data={data}
            onSelectService={handleSelectService}
            onSelectConnection={handleSelectConnection}
            onClearSelection={handleClearSelection}
          />
        </div>

        {selectedService && (
          <ServiceSheet
            service={selectedService}
            onClose={handleClearSelection}
          />
        )}
        {selectedConnection && (
          <ConnectionSheet
            connection={selectedConnection}
            graph={data}
            onClose={handleClearSelection}
          />
        )}
      </div>

      <StatusBar stats={data.stats} generatedAt={data.generatedAt} />
    </div>
  );
}
