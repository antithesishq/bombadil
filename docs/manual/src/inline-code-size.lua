-- Filter to make inline code smaller to match code blocks

function Code(el)
  if FORMAT:match 'latex' then
    -- Wrap the code element in a size-changing group with 0.95 scaling
    -- Return a list: opening brace with \relscale, the code element itself, closing brace
    return {
      pandoc.RawInline('latex', '{\\relscale{0.95}'),
      el,
      pandoc.RawInline('latex', '}')
    }
  end
  return el
end
