export class Time {
  constructor(private timestamp_ms: number) {}

  is_before(other: Time) {
    return this.timestamp_ms < other.timestamp_ms;
  }

  plus(duration: Duration) {
    return new Time(this.timestamp_ms + duration.milliseconds);
  }

  valueOf() {
    return this.timestamp_ms;
  }
}

export type TimeUnit = "milliseconds" | "seconds";

export class Duration {
  constructor(
    private n: number,
    private unit: TimeUnit,
  ) {}

  get milliseconds(): number {
    switch (this.unit) {
      case "milliseconds":
        return this.n;
      case "seconds":
        return this.n * 1000;
    }
  }
}
