import { always, type Formula } from "@antithesishq/bombadil";
import { extract } from "@antithesishq/bombadil/browser";

const resourceMetrics = [
  "js_heap_used",
  "js_heap_total",
  "dom_nodes",
  "js_event_listeners",
  "layout_objects",
] as const;

// Resource metric to watch.
export type ResourceMetric = (typeof resourceMetrics)[number];

export interface ResourceLeakOptions {
  // Metric to watch.
  metric: ResourceMetric;
  // Max growth allowed across any window. Size in bytes for heap metrics,
  // raw count otherwise.
  growthLimit: number;
  // Length of the sliding window, in milliseconds.
  windowMillis: number;
}

// Property that fails when the resource identified by `metric` grows by more
// than `growthLimit` across any sliding window of `windowMillis`. See the manual
// for tuning guidance.
export function noResourceLeak({
  metric,
  growthLimit,
  windowMillis,
}: ResourceLeakOptions): Formula {
  if (
    typeof growthLimit !== "number" ||
    isNaN(growthLimit) ||
    growthLimit <= 0
  ) {
    throw new Error(`invalid growthLimit: ${growthLimit}`);
  }
  if (
    typeof windowMillis !== "number" ||
    isNaN(windowMillis) ||
    windowMillis <= 0
  ) {
    throw new Error(`invalid windowMillis: ${windowMillis}`);
  }
  if (!resourceMetrics.includes(metric)) {
    throw new Error(`invalid metric: ${metric}`);
  }

  const samples: { timestamp: number; value: number }[] = [];

  const window = extract((state) => {
    // CDP reports timestamp in seconds; convert to ms.
    const timestamp = state.resources.timestamp * 1000;
    const value = state.resources[metric];
    samples.push({ timestamp: timestamp, value: value });

    // Drop samples older than the window, keeping the one just before it as
    // the baseline to measure growth against.
    const cutoff = timestamp - windowMillis;
    while (samples.length > 2 && samples[1]!.timestamp <= cutoff) {
      samples.shift();
    }

    const baseline = samples[0]!.value;
    return { value, baseline };
  });

  return always(
    () => window.current.value - window.current.baseline <= growthLimit,
  );
}
