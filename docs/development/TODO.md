* browser tests
  * check external links?
  * add LTL specs support
  * detect changes using mutation observers?
  * "quiescence checker": debounce outbound network request events and DOM update events and trigger
    a new state once settled (as opposed to fixed timeouts after actions), with some max timeout too
    to avoid getting stuck
  * fix click action within shadow root (some point offset?)
  * allow "no actions available" for a short period of time before failing, for instance to allow for splash and loading screens
    
* electron
  * handle file pickers
* instrumentation
  * rewrite/produce sourcemaps (or at least drop the directives from instrumented sources, as
    they'll be incorrect

# ideas

* concurrent testing with:
  * multiple independent browsers
  * multiple tabs in a single browser
* faults:
  * paused/blurred tab
  * network request reordering, delays, failures, etc (not necessary with antithesis fault injector?)
  * clear cookies, application state, etc
