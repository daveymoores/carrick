"use client";

import { useState } from "react";
import { useRouter } from "next/navigation";
import { Button } from "@/components/ui/button";

export default function Home() {
  const [org, setOrg] = useState("");
  const router = useRouter();

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (org.trim()) {
      router.push(`/org/${encodeURIComponent(org.trim())}`);
    }
  }

  return (
    <div className="min-h-screen flex flex-col items-center justify-center gap-8">
      <div className="text-center space-y-2">
        <h1 className="text-3xl font-bold text-zinc-50">Carrick Graph</h1>
        <p className="text-zinc-400">
          Visualize cross-repo API relationships
        </p>
      </div>

      <form onSubmit={handleSubmit} className="flex gap-2">
        <input
          type="text"
          placeholder="GitHub org name..."
          value={org}
          onChange={(e) => setOrg(e.target.value)}
          className="bg-zinc-800 border border-zinc-700 rounded-md px-4 py-2 text-zinc-100 placeholder:text-zinc-500 focus:outline-none focus:ring-2 focus:ring-zinc-600 w-64"
        />
        <Button type="submit" disabled={!org.trim()}>
          View Graph
        </Button>
      </form>
    </div>
  );
}
