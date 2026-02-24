# The Bombadil Changelog

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
