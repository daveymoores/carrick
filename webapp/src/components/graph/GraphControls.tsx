import { Button } from "@/components/ui/button";
import { Maximize, ZoomIn, ZoomOut, RotateCcw } from "lucide-react";

interface GraphControlsProps {
  onZoomIn: () => void;
  onZoomOut: () => void;
  onFit: () => void;
  onResetLayout: () => void;
}

export function GraphControls({
  onZoomIn,
  onZoomOut,
  onFit,
  onResetLayout,
}: GraphControlsProps) {
  return (
    <div className="absolute bottom-4 right-4 flex flex-col gap-1 bg-zinc-900 border border-zinc-800 rounded-lg p-1 shadow-lg">
      <Button variant="ghost" size="icon" onClick={onZoomIn} title="Zoom in">
        <ZoomIn className="h-4 w-4" />
      </Button>
      <Button variant="ghost" size="icon" onClick={onZoomOut} title="Zoom out">
        <ZoomOut className="h-4 w-4" />
      </Button>
      <Button variant="ghost" size="icon" onClick={onFit} title="Fit to screen">
        <Maximize className="h-4 w-4" />
      </Button>
      <Button variant="ghost" size="icon" onClick={onResetLayout} title="Reset layout">
        <RotateCcw className="h-4 w-4" />
      </Button>
    </div>
  );
}
