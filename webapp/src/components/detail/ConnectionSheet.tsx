"use client";

import type { Connection, GraphResponse } from "@/types/graph";
import { Badge } from "@/components/ui/badge";
import { getMethodColor } from "@/lib/graph-transform";
import { X, ArrowRight } from "lucide-react";
import { Button } from "@/components/ui/button";

interface ConnectionSheetProps {
  connection: Connection;
  graph: GraphResponse;
  onClose: () => void;
}

export function ConnectionSheet({
  connection,
  graph,
  onClose,
}: ConnectionSheetProps) {
  const fromService = graph.services.find(
    (s) => s.repoName === connection.fromService,
  );
  const toService = graph.services.find(
    (s) => s.repoName === connection.toService,
  );

  // Find the actual call and endpoint
  const call = fromService?.calls.find((c) => c.id === connection.from);
  const endpoint = toService?.endpoints.find((e) => e.id === connection.to);

  const statusVariant =
    connection.typeStatus === "typed"
      ? "success"
      : connection.typeStatus === "mismatch"
        ? "destructive"
        : "secondary";

  return (
    <div className="w-80 h-full bg-zinc-900 border-l border-zinc-800 overflow-y-auto">
      <div className="p-4 border-b border-zinc-800 flex items-center justify-between">
        <h2 className="text-lg font-semibold text-zinc-50">Connection</h2>
        <Button variant="ghost" size="icon" onClick={onClose}>
          <X className="h-4 w-4" />
        </Button>
      </div>

      <div className="p-4 space-y-4">
        {/* Status */}
        <Badge variant={statusVariant}>{connection.typeStatus}</Badge>

        {/* From → To */}
        <div className="space-y-3">
          <div className="bg-zinc-800/50 rounded p-3">
            <p className="text-[10px] text-zinc-500 uppercase tracking-wide mb-1">
              Consumer
            </p>
            <p className="text-sm text-zinc-300 font-medium">
              {connection.fromService}
            </p>
            {call && (
              <div className="mt-1 flex items-center gap-2 text-xs font-mono">
                <span style={{ color: getMethodColor(call.method) }}>
                  {call.method}
                </span>
                <span className="text-zinc-400">{call.targetUrl}</span>
              </div>
            )}
          </div>

          <div className="flex justify-center">
            <ArrowRight className="h-4 w-4 text-zinc-600" />
          </div>

          <div className="bg-zinc-800/50 rounded p-3">
            <p className="text-[10px] text-zinc-500 uppercase tracking-wide mb-1">
              Producer
            </p>
            <p className="text-sm text-zinc-300 font-medium">
              {connection.toService}
            </p>
            {endpoint && (
              <div className="mt-1 flex items-center gap-2 text-xs font-mono">
                <span style={{ color: getMethodColor(endpoint.method) }}>
                  {endpoint.method}
                </span>
                <span className="text-zinc-400">{endpoint.path}</span>
              </div>
            )}
          </div>
        </div>

        {/* Type detail */}
        {connection.typeDetail && (
          <div>
            <h3 className="text-sm font-medium text-zinc-300 mb-2">
              Type Information
            </h3>
            <div className="bg-zinc-800/50 rounded p-3 space-y-2 text-xs">
              {connection.typeDetail.reason && (
                <p className="text-zinc-400">{connection.typeDetail.reason}</p>
              )}
              {connection.typeDetail.producerAlias && (
                <div>
                  <span className="text-zinc-500">Producer type: </span>
                  <code className="text-zinc-300">
                    {connection.typeDetail.producerAlias}
                  </code>
                  {connection.typeDetail.producerExplicit && (
                    <Badge variant="outline" className="ml-1 text-[10px] px-1 py-0">
                      explicit
                    </Badge>
                  )}
                </div>
              )}
              {connection.typeDetail.consumerAlias && (
                <div>
                  <span className="text-zinc-500">Consumer type: </span>
                  <code className="text-zinc-300">
                    {connection.typeDetail.consumerAlias}
                  </code>
                  {connection.typeDetail.consumerExplicit && (
                    <Badge variant="outline" className="ml-1 text-[10px] px-1 py-0">
                      explicit
                    </Badge>
                  )}
                </div>
              )}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
