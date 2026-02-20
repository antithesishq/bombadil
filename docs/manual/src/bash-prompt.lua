-- Filter to handle bash prompts ($) in code blocks
-- Makes the prompt visible but non-selectable in HTML and styled in PDF

function CodeBlock(el)
  -- Only process bash code blocks
  if not el.classes:includes('bash') then
    return el
  end

  local lines = {}
  for line in el.text:gmatch("([^\n]*)\n?") do
    if line ~= "" or el.text:sub(-1) == "\n" then
      table.insert(lines, line)
    end
  end

  if FORMAT:match 'html' then
    -- For HTML: wrap prompts in non-selectable spans (including the space)
    local new_lines = {}
    for _, line in ipairs(lines) do
      if line:match("^%$ ") then
        local command = line:sub(3)
        table.insert(new_lines, '<span class="prompt">$ </span>' .. command)
      else
        table.insert(new_lines, line)
      end
    end
    -- Use <br> tags instead of actual newlines to avoid indentation issues
    local new_text = table.concat(new_lines, "<br>")
    return pandoc.RawBlock('html', '<pre class="bash"><code>' .. new_text .. '</code></pre>')

  elseif FORMAT:match 'latex' then
    -- For LaTeX: use listings with escapechar to color the prompt (including the space)
    local latex_lines = {}
    for _, line in ipairs(lines) do
      if line:match("^%$ ") then
        local command = line:sub(3)
        -- Use @ as escape character for injecting textcolor
        table.insert(latex_lines, '@\\textcolor{gray}{\\$ }@' .. command)
      else
        table.insert(latex_lines, line)
      end
    end
    local latex_text = table.concat(latex_lines, '\n')
    return pandoc.RawBlock('latex', '\\begin{lstlisting}[language=bash,escapechar=@]\n' .. latex_text .. '\n\\end{lstlisting}')

  else
    -- For other formats (epub, plain text), keep as-is
    return el
  end
end
