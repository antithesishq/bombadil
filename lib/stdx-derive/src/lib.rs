use proc_macro::{Delimiter, TokenStream, TokenTree};

/// Derives `pub const VARIANT_COUNT: usize` on an enum.
#[proc_macro_derive(VariantCount)]
pub fn derive_variant_count(input: TokenStream) -> TokenStream {
    match variant_count_impl(input) {
        Ok(output) => output,
        Err(message) => {
            format!("compile_error!({message:?});").parse().unwrap()
        }
    }
}

fn variant_count_impl(input: TokenStream) -> Result<TokenStream, String> {
    let tokens: Vec<TokenTree> = input.into_iter().collect();
    let enum_index = tokens
        .iter()
        .position(
            |t| matches!(t, TokenTree::Ident(i) if i.to_string() == "enum"),
        )
        .ok_or("VariantCount can only be derived for enums")?;
    let name = match tokens.get(enum_index + 1) {
        Some(TokenTree::Ident(name)) => name.to_string(),
        _ => return Err("expected enum name".into()),
    };

    let mut rest = &tokens[enum_index + 2..];
    let mut params = Vec::new();
    if is_punct(rest.first(), '<') {
        let close = matching_angle(rest).ok_or("unbalanced generics")?;
        params = split_top_level(&rest[1..close]);
        rest = &rest[close + 1..];
    }
    let (body, where_clause) = match rest.split_last() {
        Some((TokenTree::Group(g), where_clause))
            if g.delimiter() == Delimiter::Brace =>
        {
            (g.stream(), where_clause)
        }
        _ => return Err("expected enum body".into()),
    };

    let body: Vec<TokenTree> = body.into_iter().collect();
    let count = body
        .split(|t| is_punct(Some(t), ','))
        .filter(|variant| !variant.is_empty())
        .count();

    let impl_params: Vec<String> = params
        .iter()
        .map(|p| {
            let end = p
                .iter()
                .position(|t| is_punct(Some(t), '='))
                .unwrap_or(p.len());
            stream(&p[..end])
        })
        .collect();
    let type_args: Vec<String> = params
        .iter()
        .map(|p| match p {
            [TokenTree::Punct(q), lifetime, ..] if q.as_char() == '\'' => {
                format!("'{lifetime}")
            }
            [TokenTree::Ident(c), ident, ..] if c.to_string() == "const" => {
                ident.to_string()
            }
            [ident, ..] => ident.to_string(),
            [] => String::new(),
        })
        .collect();

    Ok(format!(
        "impl<{}> {name}<{}> {} {{ pub const VARIANT_COUNT: usize = {count}; }}",
        impl_params.join(", "),
        type_args.join(", "),
        stream(where_clause),
    )
    .parse()
    .unwrap())
}

fn is_punct(token: Option<&TokenTree>, ch: char) -> bool {
    matches!(token, Some(TokenTree::Punct(p)) if p.as_char() == ch)
}

fn angle_delta(tokens: &[TokenTree], i: usize) -> i32 {
    if is_punct(tokens.get(i), '<') {
        1
    } else if is_punct(tokens.get(i), '>')
        && !(i > 0 && is_punct(tokens.get(i - 1), '-'))
    {
        -1
    } else {
        0
    }
}

fn matching_angle(tokens: &[TokenTree]) -> Option<usize> {
    let mut depth = 0;
    for i in 0..tokens.len() {
        depth += angle_delta(tokens, i);
        if depth == 0 {
            return Some(i);
        }
    }
    None
}

fn split_top_level(tokens: &[TokenTree]) -> Vec<&[TokenTree]> {
    let mut parts = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    for i in 0..tokens.len() {
        depth += angle_delta(tokens, i);
        if depth == 0 && is_punct(tokens.get(i), ',') {
            parts.push(&tokens[start..i]);
            start = i + 1;
        }
    }
    parts.push(&tokens[start..]);
    parts.retain(|p| !p.is_empty());
    parts
}

fn stream(tokens: &[TokenTree]) -> String {
    tokens.iter().cloned().collect::<TokenStream>().to_string()
}
