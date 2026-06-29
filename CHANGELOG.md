# The Bombadil Changelog

## Unreleased

Major updates:

* Add `--cookie NAME=VALUE` option to set real browser cookies before testing, scoped to the origin (useful for client-side auth flows that read cookies, unlike `--header`)

## 0.6.1

# Major updates:

* Adapt the README and manual for multiple drivers (#215, #216)
* Refuse to silently append to an existing trace in `--output-path` (#204)

Bug fixes:

* Fix timeline tick label scaling (#171) (#196)
* Fix llms text generation in release workflow (#220)

## 0.6.0

Major updates:

* Terminal testing with the full specification language (#190, #210, #206)
* Add default properties and actions for terminal (#205)
* Add timeout for terminal programs that output forever (#201)
* Support custom elements with slots (#200)
* Browser: added MouseDrag & SetViewPort actions (#188)
* Browser: only emit text for text-producing keys (#199)

Minor updates:

* Add `#[cfg(terminal)]` feature flag for being able to build without terminal testing support (#203)
* Use Antithesis SDK for RNG, enabling the Antithesis fuzzer to explore the state space more effectively (#202)
* Log chosen action, not last action (#186)

Bug fixes:

* Use CWD for PTY spawn (#207)
* Fix `Always` wrapper stack overflow in nested temporal formula (#209)
* Add missing terminal CLI args in documentation (#185)

Breaking changes:

* The old browser testing command `bombadil test` is now under `bombadil browser test`
* Browser-specific modules in the specification language are now under `@antithesishq/browser`

Internal:

* Decouple browser and make terminal driver first-class (#183)
* Update libghostty-vt (#184)
* Improve build times in CI with hybrid nix caching approach (#182)

## 0.5.0

Major updates:

* Skip disabled controls in default clicks generator (#175)
* Add new experimental terminal fuzzer (#162, #164, #179, #180)
* Add test case reproduction as an option to `test` and `test-external` (#177)
* Improve test performance greatly by using quiescence timers instead of fixed timeouts (#176)
* Auto-accept dialogs (#166)
* Add `--header` option to pass extra request headers (#165)

Breaking changes:

* Remove `.at()` and `time` cell (#167)

Bug fixes:

* Begin browser state machine in navigating (#178)
* Use mask for edge coverage hashing (#170)
* Shutdown the browser before awaiting state machine done (#157)

Internals:

* Extract bombadil-ltl crate (#172)
* Remove esbuild (#169)
* Centralize all versions into root Cargo.toml (#168)
* Add spec and CI testing for Inspect UI (#158)
* Bump oxc deps (#156)

## 0.4.5

Major updates:

* Release npm package with executables (#154)
* Add wait-once generator to defaults (#151)
* Make CLI exit codes consistent (#150)
* Widen default granted permissions for Chromium 145 (#149)

Bug fixes:

* Explicitly close + drop chrome on terminate() (#148)

Internals:

* Bump dependencies (#153)

## 0.4.4

* Automate release procedure with script (#145)
* Grant permissions (e.g. local network access) to Chrome from CLI (#143, #144)

## 0.4.3

Major updates:

* Support file downloads and file inputs (#139)
* Pretty-printed output rather than plain logs (#133, #138)
* Autoscroll actions list in Bombadil Inspect (#131, #132)
* Improve violation error messages (#127)
* Add --timeout option (#86)

Bug fixes:

* Unwrap nested always violations recursively (#134)
* Fix stack overflow on `BundlerError` (#125)

Internals:

* Bump chromiumoxide (#140)

## 0.4.2

* Enable `trunk build` in release (fixes broken `inspect` command)

## 0.4.1

* Expose `inspect` command in release build (#119)
* Use `use_memo` for improved scrubbing performance in Bombadil Inspect (#117)


## 0.4.0

Major updates:

* Add the *Bombadil Inspect* web UI  (#81, #94, #102, #109, #114, #112)
* Install pinned version from boa `main` branch to avoid panic (#113)
* Reduce likelihood of navigation actions in defaults (#111)
* Documentation edits and improvements (#87, #88, #92, #95, #96, #101, #106, #110)
* Improve violations rendering (#104, #108)
* Include snapshots in trace (#77)
* Add DoubleClick action (#74)

Bug fixes:

* Fix cached violation bug on continued stepping (#85)
* Fix hanging pause handling (#79)
* Instrument module scripts and forward headers (#75)

Internals:

* Remove runner channel (#78)
* clean up from nix wrapper commands (#80)
* Move AGENTS.md (#83)
* Increase integration test parallelism (#76)

Breaking changes:

* Individual default action generators are no longer exported from `@antithesishq/bombadil/defaults`
* Trace file format changes

## 0.3.2

Major updates:

* Bundle specification for execution in browser (#61)
* Support importing non-code files (#63)
* Name extractors automatically for better debugging experience (#62)
* Add `Wait` action (effectively a no-op) (#65)
* Support more key codes in `PressKey` action
* Add llms.txt to GitHub Pages release artifacts (#60)
* Make JS instrumentation configurable with CLI option (#59)

Bug fixes and small improvements:

* Pretty-print console log args in log output (#64)
* Improve error message on dependent extractor use 
* Fix link to contribution guide in the getting started page (#58)

## 0.3.1

Bug fixes and small improvements:

* Fix broken links to executables in release (#56)
* Link to manual from README (#54)

## 0.3.0

Major updates:

* Add action generators to specification language (#36)
* Publish The Bombadil Manual (#47)
* Arm64 linux builds
* Sign mac executable (#33)

Breaking changes:

* Convert all TypeScript to use camelCase (#45)

Bug fixes:

* Ignore stale action (#52)
* Use sequence expressions for instrumentation hooks (#50)
* Fix action serialization issue (#46)
* Collect a first state when running in existing target (#41)
* Handle exceptions pausing (#40)
* Fix state capture hanging on screenshot (#38)
* Don't parse non-HTML using html5ever in instrumentation (#37)
* Abort tokio task running action on timeout (#35)



## 0.2.1

* Add help messages to commands and options (#30)
* Fix errors in release procedure docs (#29)
* Rewrite macOS executable to avoid linking against Nix paths (#27)
* Update install instructions after v0.2.0 release (#25)
* Optimize builds for Bombadil version bumps, speeding up the release process (#24)


## 0.2.0

* Introduced a new specification language built on TypeScript/JavaScript, with
  linear temporal logic formulas and a standard library of reusable default
  properties. (#11, #14, #18, #20)
* Fix race condition + move timeouts into browser state machine (#22)
* New rust build setup, static linking, release flow (#21)
* Auto-formatting and clippy green (#16)

## 0.1.x

Beginnings are such delicate times.
