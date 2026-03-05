import { Runtime } from "./internal";

declare global {
  module bombadil {
    const runtime: Runtime<State>;
  }
}
