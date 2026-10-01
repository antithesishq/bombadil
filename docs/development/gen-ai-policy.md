# Bombadil Generative AI Policy

*This is a living document that might change over time.*

## Purpose

In order to maintain high-quality code, documentation, and communication within the Bombadil project, every collaborator must adhere to the following policy. It helps us build a community where it makes sense to invest time into new contributors and their work, and to not erode trust and burn out maintainers.

To quote [Contributor Poker](https://kristoff.it/blog/contributor-poker-and-ai/) by Loris Cro:

> Contributing to an open source project is an iterated game and the majority of the value that a contributor can bring to a project lies in the later iterations. In other words, you initially invest some energy (i.e. place a bet) to onboard a new contributor, and you hope that later on that relationship starts paying you back as the contributor becomes more trusted and prolific.

The following rules primarily apply to the act of creating or modifying source code or text that is to be read and reviewed by maintainers.

Using LLMs to find mistakes, bugs, or to critique your own work before submitting it, is fine as long as you don't use it as a substitute for thinking. Verify claims and facts of your chat sessions before passing them onto others.

## Code

Generative AI is allowed in the context of coding, if thoroughly reviewed by the responsible author first, and if clearly stated when submitted for review by others.

Code comments, both regular ones and "doc comments", are an exception because these are primarily for humans to read and understand, and because LLMs notoriously spew out meaningless comments. Treat comments as "Documentation" as described below.

Some useful questions to ask yourself before submitting a pull request:

- "Does this follow the style of the project overall?" (note that we don't yet have a formal style guide, but we should fix that!)
- "Would I have written this code myself?"
- "Would I have wanted to review this code in a pull request?"

If you submit a pull request with generated source code, note this at the top of the description, e.g. "Code generated with Claude Code" or "Code partially produced by Codex".

This includes all program and build source code, such as Rust, TypeScript, Nix, and more.

## Documentation

The documentation of Bombadil, i.e. the manual, the change log, commit messages, internal Markdown files in the repository, various texts across package repositories and websites, must be written by humans. We take pride in the quality of our documentation, and do not allow such text to be generated.

Again, research and review in the context of documentation is fine, but *we* write the documentation, and *we* are responsible for all the thinking and empathy that goes into it.

## Communication

Similar to documentation, using LLMs to generate communication is not allowed, including GitHub issues, pull requests, Discord messages, and other human-to-human communication within the project. If you embed snippets of generated text in otherwise human-written text, put it in a `details` block:

    <details>
      <summary>Claude's analysis of the bug</summary>
      That changes everything.
    </details>

## Review

As noted above, using an LLM to review your own work is OK if done carefully and not with blind trust in its output. However, as a maintainer reviewing others' work, the point is that *you* review the work and provide thoughtful feedback. LLMs cannot be a substitute for thoroughly reading and understanding a change.

## Applicability

Quoting the [LLM Usage Policy](https://forge.rust-lang.org/policies/llm-usage.html#:~:text=Using%20LLMs%20while%20working%20on,of%20creating%20a%20strong%20community.) from the Rust project:

> We are aware that many clauses in this policy are unenforceable. Our goal is not to catch every violation \[...\]. Instead, our goal is to remove plausible deniability: to force a choice between following the policy and intentionally violating it.

Maintainers may close issues or pull requests based on this policy, or request revisions, at their own discretion.

Also note that while we of course can't apply this retroactively, we can try to clean up any dark corners that have questionable code quality, in the spirit of this policy's purpose.
