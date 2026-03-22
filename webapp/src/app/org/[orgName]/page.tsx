"use client";

import { use, useState, useCallback } from "react";
import { useQuery } from "@tanstack/react-query";
import { fetchGraph } from "@/lib/api";
import { GraphCanvas } from "@/components/graph/GraphCanvas";
import { ServiceSheet } from "@/components/detail/ServiceSheet";
import { ConnectionSheet } from "@/components/detail/ConnectionSheet";
import { SharePopover } from "@/components/SharePopover";
import { StatusBar } from "@/components/StatusBar";
import { Loader2 } from "lucide-react";
import type { Service, Connection } from "@/types/graph";

export default function OrgGraphPage({
  params,
}: {
  params: Promise<{ orgName: string }>;
}) {
  const { orgName } = use(params);
  const [selectedService, setSelectedService] = useState<Service | null>(null);
  const [selectedConnection, setSelectedConnection] =
    useState<Connection | null>(null);

  const { data, isLoading, error } = useQuery({
    queryKey: ["graph", orgName],
    queryFn: () => fetchGraph(orgName),
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

  if (isLoading) {
    return (
      <div className="min-h-screen flex items-center justify-center">
        <Loader2 className="h-8 w-8 animate-spin text-zinc-400" />
      </div>
    );
  }

  if (error || !data) {
    return (
      <div className="min-h-screen flex items-center justify-center text-zinc-400">
        <p>Failed to load graph for {orgName}</p>
      </div>
    );
  }

  return (
    <div className="h-screen flex flex-col">
      {/* Header */}
      <header className="flex items-center justify-between px-4 py-3 bg-zinc-900 border-b border-zinc-800">
        <div className="flex items-center gap-3">
          <h1 className="text-lg font-semibold text-zinc-50">Carrick Graph</h1>
          <span className="text-sm text-zinc-400">{orgName}</span>
        </div>
        <SharePopover org={orgName} />
      </header>

      {/* Main content */}
      <div className="flex flex-1 overflow-hidden">
        {/* Graph area */}
        <div className="flex-1 relative">
          <GraphCanvas
            data={data}
            onSelectService={handleSelectService}
            onSelectConnection={handleSelectConnection}
            onClearSelection={handleClearSelection}
          />
        </div>

        {/* Detail panel */}
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

      {/* Status bar */}
      <StatusBar stats={data.stats} generatedAt={data.generatedAt} />
    </div>
  );
}
