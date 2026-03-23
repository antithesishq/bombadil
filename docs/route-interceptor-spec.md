# Route Interceptor - Design Specification

## Background

Bombadil users need the ability to mock or intercept HTTP requests during browser testing, similar to Playwright's `page.route()`. This allows specs to control API responses, simulate error conditions, and test frontend behavior against deterministic backend data.

## Conversation with Maintainer

> **haznai**: having never worked on bombadil code, can you verify that this matches your vision of how it should work? i think once i've understood the general communication infrastructure i'm good to go here. from what i understand, i'm not really touching anything related to actions/properties, right? I just register my js dynamic routing code in the BOA engine through spec.ts. The browser driver in rust just manages which dynamic code (js) is loaded for which route (exported from js/ts, rust native)
>
> **maintainer**: That looks directionally correct. You can probably define the Route class in `internal.ts`, it's just going to be a small data holder that can be instanceof-checked. The thing that matches routes and then executes the custom routing callbacks is going to be the Verifier, I think. That would be the thing that implements the Router trait. Also this means that you have to do some plumbing in the `worker.rs` file (which is a concurrency-safe handle to the single-threaded verifier). The browser driver is responsible for intercepting requests (it already does this!) and invoking the Router, which may or may not match+modify that request. That way most of the complexity stays in the Verifier that owns the Route objects.

## Architecture Diagrams

### Component Overview

```
+--------------------------------------------------+
|           User spec file (spec.ts)               |
+--------------------------------------------------+
                      | import
+--------------------------------------------------+
|          @antithesishq/bombadil                  |
|                                                  |
|  index.ts       internal.ts      actions.ts      |
|  Barrel         Route class      Action gen.     |
|  exports        (NEW)                            |
+--------------------------------------------------+
                      | bundled by
+--------------------------------------------------+
|             OXC bundler (Rust)                   |
|                                                  |
|  Resolve    Rewrite (ESM->CJS)    Bundle (IIFE) |
+--------------------------------------------------+
                      | evaluated in
+--------------------------------------------------+
|             Boa engine (Rust)                    |
|                                                  |
|  Eval          Exports             Scan          |
|  Executes      + route field       + instanceof  |
|  bundle        (NEW)               Route (NEW)   |
+--------------------------------------------------+
```

### Sequence Diagram

```
Spec        Verifier        Driver          CDP         Browser
 |              |              |              |            |
 |-- eval, scan exports ------>|              |            |
 |-- Route { url, handler } -->|              |            |
 |              |              |              |            |
 |              |-- register_routes() ------->|            |
 |              |              |-- Fetch.enable() ------->|
 |              |              |<------------ OK ---------|
 |              |              |              |            |
 |              |    RUNTIME (REQUEST INTERCEPTED)        |
 |              |              |              |<-- GET /api/data
 |              |              |<-- requestPaused --------|
 |              |              |              |            |
 |              |              |-- Router.route(req) ---->|
 |              |              |              |            |
 |   +--[route.fulfill()]------+              |            |
 |   |         |              |-- fulfillRequest -------->|
 |   |         |              |              |            |
 |   +--[route.continue()]----+              |            |
 |   |         |              |-- continueRequest ------->|
 |   |         |              |              |            |
 |   +--[route.abort()]-------+              |            |
 |   |         |              |-- failRequest ----------->|
 |   +---------+              |              |            |
 |              |              |              |            |
 |              |    PASSTHROUGH (NO MATCH)                |
 |              |              |              |<-- GET /other
 |              |              |<-- requestPaused --------|
 |              |              |-- continueRequest ------>|
 |              |              |              |-- original continues
```

## Design

### Core Concept

Following the Playwright `page.route()` API, a Route holds a **URL matcher** and a **callback function**. The callback receives a route handle and the intercepted request, and decides what to do: fulfill with a custom response, continue to the network (optionally with modifications), or abort.

### 1. TypeScript: Route Class (`internal.ts`)

Per the maintainer's guidance, `Route` is defined in `internal.ts` as a small data holder that can be instanceof-checked during export scanning. Re-exported from `index.ts`.

```typescript
// internal.ts

export interface RouteRequest {
  url: string;
  method: string;
  headers: Record<string, string>;
  postData?: string;
}

export interface FulfillOptions {
  status?: number;
  headers?: Record<string, string>;
  body?: string;
}

export interface ContinueOptions {
  url?: string;
  method?: string;
  headers?: Record<string, string>;
  postData?: string;
}

export type RouteAction =
  | { type: "fulfill"; options: FulfillOptions }
  | { type: "continue"; options?: ContinueOptions }
  | { type: "abort" };

export class RouteHandle {
  private _action: RouteAction | null = null;

  fulfill(options: FulfillOptions = {}): void {
    this._action = { type: "fulfill", options };
  }

  continue(options: ContinueOptions = {}): void {
    this._action = { type: "continue", options };
  }

  abort(): void {
    this._action = { type: "abort" };
  }

  /** @internal */
  get action(): RouteAction {
    if (!this._action) {
      // Default: continue unmodified (passthrough)
      return { type: "continue" };
    }
    return this._action;
  }
}

export class Route {
  constructor(
    public url: string,
    public handler: (route: RouteHandle, request: RouteRequest) => void,
  ) {}
}
```

```typescript
// index.ts (additions)
export {
  Route,
  type RouteRequest,
  type FulfillOptions,
  type ContinueOptions,
} from "@antithesishq/bombadil/internal";
```

### 2. Rust: Router Trait

A trait that the browser driver calls when it intercepts a request. Returns an enum describing what to do.

```rust
// Likely in a new file: specification/router.rs or in verifier.rs

/// What the user's route handler decided to do with the request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RouteAction {
    /// Fulfill with a custom response (short-circuit the network).
    Fulfill {
        status: u16,
        headers: BTreeMap<String, String>,
        body: String,
    },
    /// Continue to the network, optionally with modifications.
    Continue {
        url: Option<String>,
        method: Option<String>,
        headers: Option<BTreeMap<String, String>>,
        post_data: Option<String>,
    },
    /// Abort the request.
    Abort,
}

/// Request data passed from the browser driver to the Router.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterceptedRequest {
    pub url: String,
    pub method: String,
    pub headers: BTreeMap<String, String>,
    pub post_data: Option<String>,
}

/// Trait implemented by the Verifier: match+execute route callbacks.
pub trait Router {
    fn route(&mut self, request: &InterceptedRequest) -> Result<Option<RouteAction>>;
}
```

`None` = no route matched, browser should `continueRequest`.
`Some(RouteAction)` = a route matched, browser applies the action.

### 3. Rust: Verifier implements Router (`verifier.rs`)

The Verifier stores `Route` JS objects from export scanning. When `route()` is called:

1. Iterate stored routes in registration order
2. Match the request URL against each route's `url` pattern
3. On first match: create a `RouteHandle` JS object, create a `RouteRequest` JS object from `InterceptedRequest`, call the handler callback in Boa
4. Read back the `RouteHandle.action` property, deserialize to `RouteAction`
5. Return `Some(action)`
6. If no match: return `None`

```rust
impl Router for Verifier {
    fn route(&mut self, request: &InterceptedRequest) -> Result<Option<RouteAction>> {
        for stored_route in &self.routes {
            if url_matches(&stored_route.url_pattern, &request.url) {
                // Construct RouteHandle and RouteRequest in Boa
                // Call stored_route.handler.call(route_handle, route_request)
                // Read route_handle.action
                // Deserialize and return
                return Ok(Some(action));
            }
        }
        Ok(None)
    }
}
```

### 4. Rust: Worker Plumbing (`worker.rs`)

Add a `Route` command that sends `InterceptedRequest` to the verifier thread and gets back `Option<RouteAction>`:

```rust
enum Command {
    // ... existing ...
    Route {
        request: InterceptedRequest,
        reply: oneshot::Sender<Result<Option<RouteAction>, SpecificationError>>,
    },
}
```

The `VerifierWorker` exposes an async `route()` method. The browser driver calls this when it intercepts a request.

### 5. Rust: Browser Driver (`browser.rs` / `browser/instrumentation.rs`)

The browser driver already intercepts requests via the CDP `Fetch` domain. Modify the interception to:

1. Enable `Fetch` at `RequestStage::Request` for all resource types (in addition to the existing Response-stage interception for instrumentation)
2. On `requestPaused` at Request stage: call `Router.route(request)` via the worker
3. Apply the `RouteAction`:
   - `Fulfill` -> `Fetch.fulfillRequest`
   - `Continue` -> `Fetch.continueRequest` (with optional modifications)
   - `Abort` -> `Fetch.failRequest`
   - `None` (no match) -> `Fetch.continueRequest` (unmodified)

The browser accepts the Router as a trait object (or the `VerifierWorker` directly, since it wraps the channel).

### 6. Rust: Export Scanning (`js.rs`)

Add `route: JsValue` to `BombadilExports`. During scanning, `instanceof Route` identifies route exports. Store the JS objects (with their `url` and `handler` properties) in the Verifier for later callback invocation.

## User-Facing API

```typescript
import { actions, always, extract, Route } from "@antithesishq/bombadil";

// Fulfill: mock an API with a static response
export const mockUsers = new Route("/api/users", (route, request) => {
  route.fulfill({
    status: 200,
    headers: { "content-type": "application/json" },
    body: JSON.stringify([{ id: 1, name: "Alice" }, { id: 2, name: "Bob" }]),
  });
});

// Continue: modify a request before it hits the network
export const rewriteOfficeJs = new Route("/office.js", (route, request) => {
  route.continue({ url: request.url + "?instrumented=true" });
});

// Abort: block a request entirely
export const blockAnalytics = new Route("*/analytics/*", (route, request) => {
  route.abort();
});

// Passthrough: handler that does nothing = continue unmodified
export const logRequests = new Route("/api/*", (route, request) => {
  console.log(`intercepted: ${request.method} ${request.url}`);
  // No route.fulfill/continue/abort called -> defaults to continue
});

// Existing spec features work alongside routes
export const _actions = actions(() => []);
export const usersLoad = always(() => /* ... */);
```

## Data Flow (per intercepted request)

```
Browser (CDP requestPaused)
    |
    v
VerifierWorker.route(InterceptedRequest)  [async, via mpsc channel]
    |
    v
Verifier.route(request)                   [on dedicated Boa thread]
    | - iterate stored Route objects
    | - match URL pattern
    | - call JS handler(routeHandle, request) in Boa
    | - read routeHandle.action
    |
    v
Option<RouteAction>                        [sent back via oneshot]
    |
    v
Browser applies:
    Fulfill  -> Fetch.fulfillRequest
    Continue -> Fetch.continueRequest
    Abort    -> Fetch.failRequest
    None     -> Fetch.continueRequest (passthrough)
```

## Files Changed

| File | Change |
|------|--------|
| `specification/internal.ts` | Add `Route`, `RouteHandle`, `RouteRequest`, etc. |
| `specification/index.ts` | Re-export route types |
| `specification/js.rs` | Add `route` field to `BombadilExports` |
| `specification/verifier.rs` | Store Route JS objects, implement `Router` trait, URL matching |
| `specification/worker.rs` | Add `Route` command, `route()` async method |
| `browser/instrumentation.rs` | Request-stage interception, call Router, apply RouteAction |
| `browser.rs` | Accept Router / wire up interception with worker |
| `runner.rs` | Pass worker (as Router) to browser setup |

## Open Questions

1. **Fetch domain conflict**: Both `instrument_js_coverage` (Response stage) and route interception (Request stage) call `Fetch.enable()`. Need to verify Chrome merges pattern sets, or combine into a single `Fetch.enable()` call.

2. **Pattern matching**: Playwright supports string (glob), regex, and predicate. Start with string glob matching (e.g. `**/api/*`). What pattern syntax to support in v1?

3. **Ordering**: First-registered-wins (order of exports in spec). Sufficient?

4. **Error in handler**: If the JS callback throws, default to `continueRequest` (passthrough) and log the error? Or propagate as a test failure?

5. **Async handlers**: Boa is single-threaded. Every intercepted request that hits a handler round-trips through the Boa thread. If the page fires many concurrent requests, they'll be serialized. Acceptable for v1?
