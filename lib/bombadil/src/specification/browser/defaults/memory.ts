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
  // Max growth allowed across any window: bytes for heap signals, raw count otherwise.
  thresholdBytes: number;
  // Length of the sliding window, in milliseconds.
  windowMs: number;
}

//  * Opt-in property that fails when `signal` grows by more than `thresholdBytes`
//  * across any sliding window of `windowMs`. See the manual for tuning guidance.
export function memoryDoesNotLeak({
  signal = "js_heap_used",
  thresholdBytes,
  windowMs,
}: MemoryLeakOptions): Formula {
  const samples: { t: number; v: number }[] = [];

  // Extract runs once per state in time order, so window state lives here
  // rather than in the (re-evaluated) formula thunk below.
  const leaking = extract((state) => {
    // CDP reports timestamp in seconds; convert to ms.
    const t = state.resources.timestamp * 1000;
    const v = state.resources[signal];
    samples.push({ t, v });

    // Drop samples older than the window, keeping the one just before it as
    // the baseline to measure growth against.
    const cutoff = t - windowMs;
    while (samples.length > 2 && samples[1]!.t <= cutoff) {
      samples.shift();
    }

    const baseline = samples[0]!.v;
    return v - baseline > thresholdBytes;
  });

  return always(() => !leaking.current);
}
