-- Filter to handle admonitions and HTML elements for different output formats

-- Helper function to check if we're outputting to HTML
local function is_html_output()
  return FORMAT:match 'html' ~= nil
end

-- Track state for multi-block details elements
local in_details = false

function Div(el)
  -- Handle callout admonitions
  if el.classes:includes('callout') then
    if FORMAT:match 'latex' then
      -- For PDF, convert to a simple framed box with auto-generated label
      local callout_type = 'Note'
      if el.classes:includes('callout-warning') then
        callout_type = 'Warning'
      elseif el.classes:includes('callout-tip') then
        callout_type = 'Tip'
      elseif el.classes:includes('callout-important') then
        callout_type = 'Important'
      end

      -- Create a simple framed box with a label
      return {
        pandoc.RawBlock('latex', '\\begin{quote}\\noindent\\textbf{' .. callout_type .. ':}\\par\\vspace{0.3em}'),
        el,
        pandoc.RawBlock('latex', '\\end{quote}')
      }
    end
  end

  return el
end

function RawBlock(el)
  -- Only process HTML elements when NOT outputting to HTML
  if el.format == 'html' and not is_html_output() then
    local content = el.text:gsub('^%s+', ''):gsub('%s+$', '') -- trim whitespace

    -- Opening details tag
    if content:match('^<details[^>]*>$') then
      in_details = true
      if FORMAT:match 'latex' then
        return pandoc.RawBlock('latex', '\\begin{quote}')
      end
      return {}
    end

    -- Summary tag with inline text: <summary>Text here</summary>
    local summary = content:match('^<summary[^>]*>(.-)</summary>$')
    if summary then
      if FORMAT:match 'latex' then
        return pandoc.RawBlock('latex', '\\noindent{\\large\\textbf{' .. summary .. '}}\\par\\vspace{0.5em}')
      else
        return pandoc.RawBlock('markdown', '**' .. summary .. '**\n\n')
      end
    end

    -- Opening summary tag (without inline text)
    if content:match('^<summary[^>]*>$') then
      return {}
    end

    -- Closing summary tag
    if content:match('^</summary>$') then
      return {}
    end

    -- Closing details tag
    if content:match('^</details>$') then
      in_details = false
      if FORMAT:match 'latex' then
        return pandoc.RawBlock('latex', '\\end{quote}')
      end
      return {}
    end

    -- For other HTML in non-HTML output, remove it
    return {}
  end

  -- Preserve everything else as-is
  return el
end

function RawInline(el)
  -- Only strip HTML inlines when NOT outputting to HTML
  if el.format == 'html' and not is_html_output() then
    return {}
  end
  return el
end
