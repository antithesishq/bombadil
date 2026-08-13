import { always, type Formula } from "@antithesishq/bombadil";
import { extract } from "@antithesishq/bombadil/browser";

// Resource metric to watch. Heap signals are GC-noisy; DOM/listener counts are cleaner.
export type MemoryMetric =
  | "js_heap_used"
  | "js_heap_total"
  | "dom_nodes"
  | "js_event_listeners"
  | "layout_objects";

export interface MemoryLeakOptions {
  // Metric to watch. Defaults to `"js_heap_used"`.
  signal?: MemoryMetric;
  // Max growth allowed across any window. Size in bytes for heap metrics, raw count otherwise.
  growthLimit: number;
  // Length of the sliding window, in milliseconds.
  windowMillis: number;
}

// Opt-in property that fails when `signal` grows by more than `thresholdBytes`
// across any sliding window of `windowMs`. See the manual for tuning guidance.
export function memoryDoesNotLeak({
  signal = "js_heap_used",
  growthLimit,
  windowMillis,
}: MemoryLeakOptions): Formula {
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

  const samples: { timestamp: number; value: number }[] = [];

  const window = extract((state) => {
    // CDP reports timestamp in seconds; convert to ms.
    const timestamp = state.resources.timestamp * 1000;
    const value = state.resources[signal];
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
