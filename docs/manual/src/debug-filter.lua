-- Debug filter to see what Pandoc is producing

local in_summary = false

function Para(el)
  local text = pandoc.utils.stringify(el)
  io.stderr:write("Para [in_summary=" .. tostring(in_summary) .. "]: " .. text .. "\n")
  return el
end

function RawBlock(el)
  local text = el.text:gsub('^%s+', ''):gsub('%s+$', '')
  io.stderr:write("RawBlock format=" .. el.format .. " text='" .. text .. "'\n")

  -- Track summary state
  if text:match('^<summary') then
    in_summary = true
    io.stderr:write("  --> ENTERING SUMMARY\n")
  elseif text:match('^</summary>') then
    in_summary = false
    io.stderr:write("  --> EXITING SUMMARY\n")
  end

  return el
end

function RawInline(el)
  io.stderr:write("RawInline format=" .. el.format .. " text=" .. el.text .. "\n")
  return el
end

function Div(el)
  io.stderr:write("Div classes=" .. table.concat(el.classes, ",") .. "\n")
  return el
end

return {
  {Para = Para},
  {RawBlock = RawBlock},
  {RawInline = RawInline},
  {Div = Div}
}
