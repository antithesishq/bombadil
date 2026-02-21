-- Reset column widths to auto-sized defaults.
-- Pandoc assigns fixed proportional widths that cause bad wrapping.

function Table(el)
  for i, colspec in ipairs(el.colspecs) do
    colspec[2] = nil
  end
  return el
end
