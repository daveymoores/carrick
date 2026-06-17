import { useEffect } from 'react';

interface PageViewPayload {
  path: string;
  referrer: string;
}

// A browser component that reports analytics with the web-platform
// `navigator.sendBeacon` primitive (a fire-and-forget HTTP POST). There is no
// import for it — it is a browser built-in — so only the scanner's structural
// shape recognition keeps this file from being skipped before the LLM.
export function AnalyticsBeacon({ path, referrer }: PageViewPayload) {
  useEffect(() => {
    const payload = JSON.stringify({ path, referrer });

    // Relative URL.
    navigator.sendBeacon('/collect', payload);

    // Absolute URL.
    navigator.sendBeacon('https://metrics.example.com/collect', payload);
  }, [path, referrer]);

  return null;
}
