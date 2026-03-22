"use client";

import { useEffect, useRef, useCallback } from "react";
import type { Core, EventObject } from "cytoscape";
import type { GraphResponse, Service, Connection } from "@/types/graph";
import {
  transformToElements,
  getMethodColor,
  getEdgeColor,
} from "@/lib/graph-transform";

interface GraphCanvasProps {
  data: GraphResponse;
  onSelectService?: (service: Service) => void;
  onSelectConnection?: (connection: Connection) => void;
  onClearSelection?: () => void;
}

export function GraphCanvas({
  data,
  onSelectService,
  onSelectConnection,
  onClearSelection,
}: GraphCanvasProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const cyRef = useRef<Core | null>(null);

  const initGraph = useCallback(async () => {
    if (!containerRef.current) return;

    const cytoscape = (await import("cytoscape")).default;
    const coseBilkent = (await import("cytoscape-cose-bilkent")).default;

    cytoscape.use(coseBilkent);

    const elements = transformToElements(data);

    const cy = cytoscape({
      container: containerRef.current,
      elements,
      style: [
        // Service (parent) nodes
        {
          selector: 'node[nodeType="service"]',
          style: {
            label: "data(label)",
            "text-valign": "top" as const,
            "text-halign": "center" as const,
            "font-size": "14px",
            "font-weight": "bold" as const,
            "background-color": "#18181b",
            "background-opacity": 0.05,
            "border-width": 2,
            "border-color": "#3f3f46",
            "border-opacity": 0.3,
            "text-margin-y": -8,
            padding: "24px",
            shape: "roundrectangle" as const,
            color: "#a1a1aa",
          },
        },
        // Endpoint (producer) child nodes
        {
          selector: 'node[nodeType="endpoint"]',
          style: {
            label: "data(label)",
            "text-valign": "center" as const,
            "text-halign": "center" as const,
            "font-size": "11px",
            "font-family": "monospace",
            "background-color": (ele: { data: (key: string) => string }) =>
              getMethodColor(ele.data("method")),
            "background-opacity": 0.15,
            "border-width": 2,
            "border-color": (ele: { data: (key: string) => string }) =>
              getMethodColor(ele.data("method")),
            color: "#e4e4e7",
            shape: "roundrectangle" as const,
            width: "label",
            height: 32,
            padding: "8px",
          },
        },
        // Call (consumer) child nodes
        {
          selector: 'node[nodeType="call"]',
          style: {
            label: "data(label)",
            "text-valign": "center" as const,
            "text-halign": "center" as const,
            "font-size": "11px",
            "font-family": "monospace",
            "background-color": (ele: { data: (key: string) => string }) =>
              getMethodColor(ele.data("method")),
            "background-opacity": 0.08,
            "border-width": 1,
            "border-style": "dashed" as const,
            "border-color": (ele: { data: (key: string) => string }) =>
              getMethodColor(ele.data("method")),
            color: "#a1a1aa",
            shape: "roundrectangle" as const,
            width: "label",
            height: 32,
            padding: "8px",
          },
        },
        // Edges
        {
          selector: "edge",
          style: {
            width: 2,
            "line-color": (ele: { data: (key: string) => string }) =>
              getEdgeColor(ele.data("typeStatus")),
            "target-arrow-color": (ele: { data: (key: string) => string }) =>
              getEdgeColor(ele.data("typeStatus")),
            "target-arrow-shape": "triangle" as const,
            "curve-style": "bezier" as const,
            "arrow-scale": 1.2,
          },
        },
        // Hover state
        {
          selector: "node:active, node:selected",
          style: {
            "border-width": 3,
            "border-color": "#a78bfa",
          },
        },
        {
          selector: "edge:selected",
          style: {
            width: 3,
            "line-color": "#a78bfa",
            "target-arrow-color": "#a78bfa",
          },
        },
      ],
      layout: {
        name: "cose-bilkent",
        // @ts-expect-error — cose-bilkent options not in base types
        animate: "end",
        animationDuration: 500,
        nodeRepulsion: 8000,
        idealEdgeLength: 120,
        edgeElasticity: 0.1,
        nestingFactor: 0.1,
        gravity: 0.2,
        numIter: 2500,
        tile: true,
        tilingPaddingVertical: 24,
        tilingPaddingHorizontal: 24,
        fit: true,
        padding: 40,
      },
      minZoom: 0.1,
      maxZoom: 4,
      wheelSensitivity: 0.3,
    });

    // Event handlers
    cy.on("tap", 'node[nodeType="service"]', (evt: EventObject) => {
      const serviceData = evt.target.data("service");
      if (serviceData && onSelectService) onSelectService(serviceData);
    });

    cy.on("tap", 'node[nodeType="endpoint"], node[nodeType="call"]', (evt: EventObject) => {
      const parentId = evt.target.data("parent");
      const parentService = data.services.find((s) => s.id === parentId);
      if (parentService && onSelectService) onSelectService(parentService);
    });

    cy.on("tap", "edge", (evt: EventObject) => {
      const connection = evt.target.data("connection");
      if (connection && onSelectConnection) onSelectConnection(connection);
    });

    cy.on("tap", (evt: EventObject) => {
      if (evt.target === cy && onClearSelection) onClearSelection();
    });

    cyRef.current = cy;

    return () => {
      cy.destroy();
    };
  }, [data, onSelectService, onSelectConnection, onClearSelection]);

  useEffect(() => {
    const cleanup = initGraph();
    return () => {
      cleanup?.then((fn) => fn?.());
    };
  }, [initGraph]);

  return (
    <div
      ref={containerRef}
      className="w-full h-full bg-zinc-950 rounded-lg"
    />
  );
}
