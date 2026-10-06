import { Runtime } from "./internal";

declare global {
  namespace bombadil {
    const runtime: Runtime<unknown>;
  }
}
