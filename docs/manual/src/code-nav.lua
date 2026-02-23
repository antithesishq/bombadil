-- Filter to wrap code blocks in a container with a navigation bar
-- Only applies to HTML output

function CodeBlock(el)
  if not FORMAT:match 'html' then
    return el
  end

  local lang = el.classes[1] or ''

  if lang == '' then
    return el
  end

  local nav = '<nav class="code-navigation">'
      .. '<button class="copy"><span class="icon">⧉</span>Copy</button>'
      .. '<span class="name">' .. lang .. '</span>'
      .. '</nav>'

  return {
    pandoc.RawBlock('html', '<div class="code-block">'),
    pandoc.RawBlock('html', nav),
    el,
    pandoc.RawBlock('html', '</div>')
  }
end
