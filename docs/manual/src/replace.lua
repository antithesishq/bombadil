local vars = {}
local str_vars = {}

local function escape_pattern(text)
  return (text:gsub("([^%w])", "%%%1"))
end

local function get_vars (meta)
  for k, v in pairs(meta) do
    if pandoc.utils.type(v) == 'Inlines' then
      vars["%" .. k .. "%"] = {table.unpack(v)}
      str_vars["%" .. k .. "%"] = pandoc.utils.stringify(v)
    elseif pandoc.utils.type(v) == 'string' then
      vars["%" .. k .. "%"] = pandoc.Inlines(v)
      str_vars["%" .. k .. "%"] = v
    end
  end
end

local function replace (el)
  if vars[el.text] then
    return pandoc.Span(vars[el.text])
  else
    return el
  end
end

local function replace_link (el)
  for pattern, replacement in pairs(str_vars) do
    el.target = el.target:gsub(escape_pattern(pattern), replacement)
  end
  return el
end

local function replace_code (el)
  for pattern, replacement in pairs(str_vars) do
    el.text = el.text:gsub(escape_pattern(pattern), replacement)
  end
  return el
end

local function replace_code_block (el)
  for pattern, replacement in pairs(str_vars) do
    el.text = el.text:gsub(escape_pattern(pattern), replacement)
  end
  return el
end

function Pandoc(doc)
  return doc:walk { Meta = get_vars }:walk { Str = replace, Link = replace_link, Code = replace_code, CodeBlock = replace_code_block }
end
