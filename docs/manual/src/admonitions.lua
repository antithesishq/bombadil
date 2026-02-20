-- Filter to handle admonitions and HTML elements for different output formats

-- Helper function to check if we're outputting to HTML
local function is_html_output()
  return FORMAT:match 'html' ~= nil
end

-- Track state for multi-block details elements
local in_details = false
local in_summary = false

function Div(el)
  -- Handle callout admonitions
  if el.classes:includes('callout') then
    if FORMAT:match 'latex' then
      -- For PDF, wrap in admonitionbox with colored label
      local callout_type = 'NOTE'
      local callout_color = 'admonitionblue'
      local callout_icon = '\\faInfoCircle' -- Font Awesome info circle
      if el.classes:includes('callout-warning') then
        callout_type = 'WARNING'
        callout_color = 'admonitionyellow'
        callout_icon = '\\faExclamationTriangle' -- Font Awesome warning triangle
      elseif el.classes:includes('callout-tip') then
        callout_type = 'TIP'
        callout_color = 'admonitioncyan'
        callout_icon = '\\faLightbulb[regular]' -- Font Awesome lightbulb (regular style)
      elseif el.classes:includes('callout-important') then
        callout_type = 'IMPORTANT'
        callout_color = 'admonitionred'
        callout_icon = '\\faExclamationCircle' -- Font Awesome exclamation circle
      end

      -- Create a label paragraph on its own line (no colon, with icon)
      -- Add spacing between icon and label, and after the label line
      local label_para = pandoc.Para({
        pandoc.RawInline('latex',
          '\\textcolor{' .. callout_color .. '}{' .. callout_icon .. '\\hspace{0.5em}\\textbf{' .. callout_type .. '}}')
      })

      -- Add vertical space after the label
      local label_space = pandoc.RawBlock('latex', '\\vspace{0.5em}')

      -- Wrap content in colored admonition box
      local begin_box = pandoc.RawBlock('latex', '\\begin{' .. callout_color .. '}')
      local end_box = pandoc.RawBlock('latex', '\\end{' .. callout_color .. '}')

      -- Insert: begin box, label paragraph, spacing, original content, end box
      table.insert(el.content, 1, label_space)
      table.insert(el.content, 1, label_para)
      table.insert(el.content, 1, begin_box)
      table.insert(el.content, end_box)

      return el
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
        -- Use a simple structure with indentation
        return pandoc.RawBlock('latex', '')
      end
      return {}
    end

    -- Summary tag with inline text: <summary>Text here</summary>
    local summary = content:match('^<summary[^>]*>(.-)</summary>$')
    if summary then
      if FORMAT:match 'latex' then
        return pandoc.RawBlock('latex', '\\noindent\\textbf{' .. summary .. '}\\par\\vspace{0.3em}\n\\begin{quote}')
      else
        return pandoc.RawBlock('markdown', '**' .. summary .. '**\n\n')
      end
    end

    -- Opening summary tag (without inline text)
    if content:match('^<summary[^>]*>$') then
      in_summary = true
      return {}
    end

    -- Closing summary tag
    if content:match('^</summary>$') then
      in_summary = false
      if FORMAT:match 'latex' then
        return pandoc.RawBlock('latex', '\\vspace{0.3em}\n\\begin{quote}')
      end
      return {}
    end

    -- Closing details tag
    if content:match('^</details>$') then
      in_details = false
      if FORMAT:match 'latex' then
        return pandoc.RawBlock('latex', '\\end{quote}\\vspace{0.5em}')
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

function Para(el)
  -- When inside summary, make paragraph bold for LaTeX
  if in_summary and FORMAT:match 'latex' then
    return pandoc.Para(pandoc.Strong(el.content))
  end
  return el
end

function Plain(el)
  -- When inside summary, make plain text bold for LaTeX
  if in_summary and FORMAT:match 'latex' then
    return pandoc.Plain(pandoc.Strong(el.content))
  end
  return el
end
