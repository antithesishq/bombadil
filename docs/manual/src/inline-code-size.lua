-- Filter to make inline code smaller to match code blocks

function Code(el)
  if FORMAT:match 'latex' then
    -- Wrap the code element in a size-changing group with \small
    -- Return a list: opening brace with \small, the code element itself, closing brace
    return {
      pandoc.RawInline('latex', '{\\small{}'),
      el,
      pandoc.RawInline('latex', '}')
    }
  end
  return el
end
