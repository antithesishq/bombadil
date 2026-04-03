# The Bombadil Changelog

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
