* browser tests
  * check external links?
  * add LTL specs support
  * detect changes using mutation observers?
  * "quiescence checker": debounce outbound network request events and DOM update events and trigger
    a new state once settled (as opposed to fixed timeouts after actions), with some max timeout too
    to avoid getting stuck
    
* electron
  * handle file pickers
* instrumentation
  * instrument inline scripts in html?
    * see riotjs example in todomvc!
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
