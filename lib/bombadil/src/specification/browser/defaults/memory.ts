import { always, type Formula } from "@antithesishq/bombadil";
import { extract } from "@antithesishq/bombadil/browser";

/**
 * Resource metrics that can be watched for leaks. All are collected on every
 * state from Chrome DevTools `Performance.getMetrics`. `js_heap_used` /
 * `js_heap_total` are GC-noisy; `dom_nodes` and `js_event_listeners` are
 * GC-independent and usually cleaner leak signals.
 */
export type MemoryMetric =
  | "js_heap_used"
  | "js_heap_total"
  | "dom_nodes"
  | "js_event_listeners"
  | "layout_objects";

export interface MemoryLeakOptions {
  /** Metric to watch. Defaults to `"js_heap_used"`. */
  signal?: MemoryMetric;
  /**
   * Maximum growth allowed across any window of `windowMs`. In bytes for the
   * heap signals; a raw count for `dom_nodes` / `js_event_listeners` /
   * `layout_objects`.
   */
  thresholdBytes: number;
  /** Length of the sliding window, in milliseconds. */
  windowMs: number;
}

/**
 * A parameterized property that detects memory leaks in the system under test.
 *
 * Semantics — *windowed growth*: within any sliding window of `windowMs`, the
 * chosen `signal` must not grow by more than `thresholdBytes`. This flags a
 * sustained climb (a leak) while tolerating transient spikes that recede and,
 * for the heap signals, healthy GC sawtooth.
 *
 * This property is **not** part of the defaults bundle — opt in by importing it
 * and exporting a configured instance from your specification:
 *
 * ```ts
 * import { memoryDoesNotLeak } from "@antithesishq/bombadil/browser/defaults/memory";
 * export const noHeapLeak = memoryDoesNotLeak({ thresholdBytes: 5_000_000, windowMs: 10_000 });
 * export const noDomLeak = memoryDoesNotLeak({ signal: "dom_nodes", thresholdBytes: 500, windowMs: 10_000 });
 * ```
 *
 * The exported constant's name becomes the property's label in reports.
 *
 * Tuning: `windowMs` must be smaller than the run's time limit or the check
 * never engages. For `js_heap_used`, keep `thresholdBytes` above the sawtooth
 * amplitude (or prefer `dom_nodes` / `js_event_listeners`).
 */
export function memoryDoesNotLeak({
  signal = "js_heap_used",
  thresholdBytes,
  windowMs,
}: MemoryLeakOptions): Formula {
  const samples: { t: number; v: number }[] = [];

  // Extractors run exactly once per state, in time order (see `runExtractors`
  // in the runtime), so this closure is the correct place to maintain the
  // sliding window. The formula thunk below may be re-evaluated while residuals
  // are simplified, so the windowing logic must live here, not in the thunk.
  const leaking = extract((state) => {
    // CDP reports `timestamp` in seconds; convert to milliseconds to match `windowMs`.
    const t = state.resources.timestamp * 1000;
    const v = state.resources[signal];
    samples.push({ t, v });

    // Keep every sample within the window [t - windowMs, t], plus the single
    // sample just before the window starts, which serves as the "value
    // windowMs ago" baseline. Growth is measured against that baseline, so a
    // spike that has already receded is not flagged, while a sustained climb
    // is. Before a full window has elapsed the baseline is simply the first
    // observed value.
    const cutoff = t - windowMs;
    while (samples.length > 2 && samples[1]!.t <= cutoff) {
      samples.shift();
    }

    const baseline = samples[0]!.v;
    return v - baseline > thresholdBytes;
  });

  return always(() => !leaking.current);
}
