"use client";

import type { Service } from "@/types/graph";
import { Badge } from "@/components/ui/badge";
import { getMethodColor } from "@/lib/graph-transform";
import { X } from "lucide-react";
import { Button } from "@/components/ui/button";

interface ServiceSheetProps {
  service: Service;
  onClose: () => void;
}

export function ServiceSheet({ service, onClose }: ServiceSheetProps) {
  return (
    <div className="w-80 h-full bg-zinc-900 border-l border-zinc-800 overflow-y-auto">
      <div className="p-4 border-b border-zinc-800 flex items-center justify-between">
        <div>
          <h2 className="text-lg font-semibold text-zinc-50">
            {service.serviceName}
          </h2>
          <p className="text-sm text-zinc-400">{service.repoName}</p>
        </div>
        <Button variant="ghost" size="icon" onClick={onClose}>
          <X className="h-4 w-4" />
        </Button>
      </div>

      <div className="p-4 space-y-4">
        {/* Meta */}
        <div className="flex gap-2 text-xs text-zinc-500">
          {service.commitHash && (
            <span className="font-mono">{service.commitHash.slice(0, 7)}</span>
          )}
          {service.lastUpdated && (
            <span>{new Date(service.lastUpdated).toLocaleDateString()}</span>
          )}
        </div>

        {/* Endpoints */}
        <div>
          <h3 className="text-sm font-medium text-zinc-300 mb-2">
            Endpoints ({service.endpoints.length})
          </h3>
          <div className="space-y-1">
            {service.endpoints.map((ep) => (
              <div
                key={ep.id}
                className="flex items-center gap-2 px-2 py-1.5 rounded bg-zinc-800/50 text-xs font-mono"
              >
                <span
                  className="font-semibold shrink-0"
                  style={{ color: getMethodColor(ep.method) }}
                >
                  {ep.method}
                </span>
                <span className="text-zinc-300 truncate">{ep.path}</span>
                {ep.hasTypes && (
                  <Badge variant="success" className="ml-auto text-[10px] px-1.5 py-0">
                    T
                  </Badge>
                )}
              </div>
            ))}
            {service.endpoints.length === 0 && (
              <p className="text-xs text-zinc-500">No endpoints</p>
            )}
          </div>
        </div>

        {/* Calls */}
        <div>
          <h3 className="text-sm font-medium text-zinc-300 mb-2">
            API Calls ({service.calls.length})
          </h3>
          <div className="space-y-1">
            {service.calls.map((call) => (
              <div
                key={call.id}
                className="flex items-center gap-2 px-2 py-1.5 rounded bg-zinc-800/50 text-xs font-mono"
              >
                <span
                  className="font-semibold shrink-0"
                  style={{ color: getMethodColor(call.method) }}
                >
                  {call.method}
                </span>
                <span className="text-zinc-400 truncate">{call.targetUrl}</span>
                <span className="ml-auto text-zinc-600 text-[10px]">
                  {call.client}
                </span>
              </div>
            ))}
            {service.calls.length === 0 && (
              <p className="text-xs text-zinc-500">No outgoing calls</p>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
