---
title: The Bombadil Manual
---

# Introduction

Bombadil is [property-based
testing](https://antithesis.com/docs/resources/property_based_testing/) for web
UIs. It autonomously explores and validates correctness properties, *finding
harder bugs earlier*. It runs in your local developer environment, in CI, and
inside [Antithesis](https://antithesis.com/).

## Why Bombadil?

Or rather, *why property-based testing?* Because example-based testing,
especially when browser testing, is costly and limited:

* Costly, because maintaining suites of Playwright or Cypress tests takes a lot
  of work. Even in the age of AI, tests written and updated by coding agents
  can easily break and require your attention.
* Limited, in that they only test very small parts of the state space; a bunch
  of happy cases, a set of regression tests, and maybe even some error handling
  cases that are important. But what about everything else --- like all the
  stuff you or the agent didn't think about testing?

This is where property-based testing, or *fuzzing* if you will, comes into
play. By randomly and systematically searching the state space, Bombadil
behaves in ways you didn't think about testing for; unexpected sequences of
actions, weird timings, strange inputs that you forgot could be entered.

## How it works

Instead of describing "what good looks like" in terms of fixed test cases, you
express general properties of your system, defining how it should behave in all cases.
Bombadil checks each property as it explores your system in its chaotic ways,
reporting back any violations.

To test a web application using Bombadil, you write a specification in
TypeScript that exports [properties](#properties) and [action
generators](#action-generators). These can be domain-specific --- to exercise
and validate your system's logic in custom ways --- or be imported from
Bombadil's [defaults](#default-properties-and-action-generators). Bombadil
tests anything that uses the DOM, no matter how it's built. This includes
single-page apps, server-side rendered apps, and even static HTML.

Conceptually, it runs in a loop doing the following:

1. Extracts the current state from the browser
2. Checks all properties against the current state, recording violations[^exit]
3. Selects the next action based on the current state, and performs it
4. Waits for the next event (page navigation, DOM mutation, or timeout)
5. *Returns to step 1*

Bombadil itself decides what is an interesting event and when to capture state.
You provide the properties and actions, Bombadil does the rest!

[^exit]: You can also configure Bombadil to exit on the first found violation.
