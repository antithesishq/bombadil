-- Driver-conditional content filter.
-- Pass `-M driver=browser` (or `-M driver=terminal`) to pandoc; Divs and Spans
-- whose class names match a known driver are kept only when the class matches
-- the current driver, and dropped otherwise. When the gate is the only thing
-- on the wrapper, it is unwrapped so the inner content renders flat.

local known_drivers = { browser = true, terminal = true }

local function driver_class(classes)
  for _, c in ipairs(classes) do
    if known_drivers[c] then
      return c
    end
  end
  return nil
end

local function gate(el, current)
  local cls = driver_class(el.classes)
  if not cls then return nil end
  if cls ~= current then return {} end

  el.classes = el.classes:filter(function(c) return c ~= cls end)

  local plain_wrapper = #el.classes == 0
    and el.identifier == ""
    and (not el.attributes or #el.attributes == 0)
  if plain_wrapper then
    return el.content
  end
  return el
end

function Pandoc(doc)
  local current = doc.meta.driver and pandoc.utils.stringify(doc.meta.driver)
  if not current then return doc end
  if not known_drivers[current] then
    error("driver.lua: unknown driver '" .. current .. "'")
  end
  return doc:walk {
    Div = function(el) return gate(el, current) end,
    Span = function(el) return gate(el, current) end,
  }
end
