# Admonition Examples

This file demonstrates how to use admonitions and collapsible sections in your documentation.

## Admonitions

Use Pandoc's fenced div syntax with the appropriate class. The label will be added automatically:

::: {.callout .callout-note}
This is a note admonition. Use it for general information or clarifications.
:::

::: {.callout .callout-warning}
This is a warning admonition. Use it to highlight potential issues or things to be careful about.
:::

::: {.callout .callout-tip}
This is a tip admonition. Use it for helpful suggestions or best practices.
:::

::: {.callout .callout-important}
This is an important admonition. Use it for critical information that users must not miss.
:::

### Multiple Paragraphs

### Multiple Paragraphs

Admonitions can contain multiple paragraphs and other elements:

::: {.callout .callout-tip}
When writing specifications, always:

- Start with simple cases
- Add edge cases incrementally
- Document your assumptions

This helps maintain clarity and makes debugging easier.
:::

### With Code Examples

::: {.callout .callout-warning}
Never hardcode credentials in your specifications:

```typescript
// Bad
const apiKey = "sk-1234567890";

// Good
const apiKey = process.env.API_KEY;
```
:::

## Collapsible Sections (HTML only)

Use HTML `<details>` and `<summary>` elements for collapsible content. These will appear as collapsible sections in HTML output and as bold headings in PDF.

### Single Collapsible Section

For a standalone collapsible section:

<details>
<summary>Click to expand for more details</summary>

This is a single collapsible section with some content.

</details>

### Grouped Collapsible Sections (Accordion)

For multiple related collapsible sections, wrap them in a `<div class="accordion">`:

<div class="accordion">
<details>
<summary>Installation via npm</summary>

```bash
npm install @antithesishq/bombadil
```

</details>
<details>
<summary>Installation via yarn</summary>

```bash
yarn add @antithesishq/bombadil
```

</details>
<details>
<summary>Installation via pnpm</summary>

```bash
pnpm add @antithesishq/bombadil
```

</details>
</div>
