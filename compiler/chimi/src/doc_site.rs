//! `chimi doc --html`: static documentation pages, one per module and an
//! index, from the same `ModuleDocumentation` the terminal form prints.
//!
//! Pages are self-contained HTML with inline styles, so the directory can be
//! opened from disk or served as it is. Doc comments are read as a small
//! subset of Markdown: paragraphs, `-` lists, ```` ``` ```` fences and
//! `inline code`; anything else shows as written.

use crate::doc::ModuleDocumentation;

/// One page to write: its path relative to the output directory, and its HTML.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocPage {
    pub path: String,
    pub html: String,
}

/// The pages for `modules`: `index.html`, listing every module with the first
/// line of its summary, and one page per module at its module path, as
/// `app/checkout.html`. Modules are listed in path order.
pub fn render_site(title: &str, modules: &[ModuleDocumentation]) -> Vec<DocPage> {
    let mut modules: Vec<&ModuleDocumentation> = modules.iter().collect();
    modules.sort_by(|left, right| left.module_path().cmp(right.module_path()));
    let mut pages = vec![DocPage {
        path: "index.html".to_owned(),
        html: index_page(title, &modules),
    }];
    pages.extend(modules.iter().map(|module| DocPage {
        path: page_path(module.module_path()),
        html: module_page(module),
    }));
    pages
}

fn page_path(module_path: &str) -> String {
    format!("{module_path}.html")
}

/// The relative link from a page at `from` (a module path, or `index`) to
/// the page at `to`.
fn link(from: &str, to: &str) -> String {
    let depth = from.matches('/').count();
    format!("{}{to}", "../".repeat(depth))
}

fn index_page(title: &str, modules: &[&ModuleDocumentation]) -> String {
    let mut body = format!("<h1>{}</h1>\n<ul class=\"modules\">\n", escape(title));
    for module in modules {
        let path = module.module_path();
        body.push_str(&format!(
            "<li><a href=\"{}\"><code>{}</code></a>",
            escape(&link("index", &page_path(path))),
            escape(path)
        ));
        if let Some(first) = module.summary().and_then(first_paragraph) {
            body.push_str(&format!(" <span>{}</span>", inline(&first)));
        }
        body.push_str("</li>\n");
    }
    body.push_str("</ul>\n");
    page(title, &body)
}

fn module_page(module: &ModuleDocumentation) -> String {
    let path = module.module_path();
    let mut body = format!(
        "<p><a href=\"{}\">Index</a></p>\n<h1>module <code>{}</code></h1>\n",
        escape(&link(path, "index.html")),
        escape(path)
    );
    if let Some(summary) = module.summary() {
        body.push_str(&markdown(summary));
    }
    for item in module.items() {
        body.push_str(&format!(
            "<section id=\"{name}\">\n<h2><a href=\"#{name}\">{kind} <code>{name}</code></a></h2>\n<pre><code>{signature}</code></pre>\n",
            name = escape(item.name()),
            kind = escape(item.kind()),
            signature = escape(item.signature()),
        ));
        if let Some(doc) = item.doc() {
            body.push_str(&markdown(doc));
        }
        body.push_str("</section>\n");
    }
    page(&format!("{path} - Chimi docs"), &body)
}

fn page(title: &str, body: &str) -> String {
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>{}</title>\n<style>{STYLE}</style>\n</head>\n<body>\n<main>\n{body}</main>\n</body>\n</html>\n",
        escape(title)
    )
}

const STYLE: &str = "\
:root{color-scheme:light dark;--fg:#1d1d1f;--muted:#5f6368;--bg:#fff;--code:#f3f4f6;--rule:#e5e7eb}\
@media (prefers-color-scheme:dark){:root{--fg:#e8eaed;--muted:#9aa0a6;--bg:#18191b;--code:#26282b;--rule:#33363a}}\
body{margin:0;background:var(--bg);color:var(--fg);font:16px/1.55 system-ui,sans-serif}\
main{max-width:52rem;margin:0 auto;padding:1.5rem 1rem 4rem}\
h1{font-size:1.6rem}h2{font-size:1.1rem;margin:2rem 0 .5rem;font-weight:600}\
h2 a{color:inherit;text-decoration:none}a{color:#2563eb}\
code{font:.92em ui-monospace,SFMono-Regular,Menlo,monospace}\
p code,li code{background:var(--code);padding:.1em .3em;border-radius:4px}\
pre{background:var(--code);padding:.75rem 1rem;border-radius:6px;overflow-x:auto}\
section{border-top:1px solid var(--rule)}ul.modules{padding-left:1.2rem}\
ul.modules span{color:var(--muted)}";

/// The first paragraph of `text` on one line, for the index, or none when
/// `text` opens with a code block.
fn first_paragraph(text: &str) -> Option<String> {
    let lines: Vec<&str> = text
        .lines()
        .take_while(|line| !line.trim().is_empty() && fence_length(line).is_none())
        .map(str::trim)
        .collect();
    (!lines.is_empty()).then(|| lines.join(" "))
}

/// The number of backticks opening `line` when it is a code fence: three or
/// more.
fn fence_length(line: &str) -> Option<usize> {
    let ticks = line.trim_start().chars().take_while(|&c| c == '`').count();
    (ticks >= 3).then_some(ticks)
}

/// The doc comment `text` as HTML: blank lines separate paragraphs, `- `
/// lines make a list, ```` ``` ```` fences make a code block, and
/// backticks mark inline code.
fn markdown(text: &str) -> String {
    let mut html = String::new();
    let mut paragraph: Vec<&str> = Vec::new();
    let mut list: Vec<String> = Vec::new();
    // The open code block's fence length and lines; a fence closes at a
    // line of at least as many backticks and nothing else.
    let mut fence: Option<(usize, Vec<&str>)> = None;
    let flush = |html: &mut String, paragraph: &mut Vec<&str>, list: &mut Vec<String>| {
        if !paragraph.is_empty() {
            html.push_str(&format!("<p>{}</p>\n", inline(&paragraph.join(" "))));
            paragraph.clear();
        }
        if !list.is_empty() {
            html.push_str("<ul>\n");
            for entry in list.iter() {
                html.push_str(&format!("<li>{}</li>\n", inline(entry)));
            }
            html.push_str("</ul>\n");
            list.clear();
        }
    };
    for line in text.lines() {
        if let Some((opening, code)) = fence.as_mut() {
            let closes = fence_length(line)
                .is_some_and(|ticks| ticks >= *opening && line.trim().len() == ticks);
            if closes {
                html.push_str(&format!(
                    "<pre><code>{}</code></pre>\n",
                    escape(&code.join("\n"))
                ));
                fence = None;
            } else {
                code.push(line);
            }
        } else if let Some(ticks) = fence_length(line) {
            flush(&mut html, &mut paragraph, &mut list);
            fence = Some((ticks, Vec::new()));
        } else if line.trim().is_empty() {
            flush(&mut html, &mut paragraph, &mut list);
        } else if let Some(entry) = line.trim_start().strip_prefix("- ") {
            if !paragraph.is_empty() {
                flush(&mut html, &mut paragraph, &mut list);
            }
            list.push(entry.to_owned());
        } else if let Some(last) = list.last_mut().filter(|_| line.starts_with(' ')) {
            // An indented line continues the last list entry.
            last.push(' ');
            last.push_str(line.trim());
        } else {
            if !list.is_empty() {
                flush(&mut html, &mut paragraph, &mut list);
            }
            paragraph.push(line.trim());
        }
    }
    if let Some((_, code)) = fence {
        // An unclosed fence shows as code to the end.
        html.push_str(&format!(
            "<pre><code>{}</code></pre>\n",
            escape(&code.join("\n"))
        ));
    }
    flush(&mut html, &mut paragraph, &mut list);
    html
}

/// One line of doc text with `code` spans; an unpaired backtick is literal.
fn inline(text: &str) -> String {
    let mut html = String::new();
    let mut rest = text;
    while let Some(open) = rest.find('`') {
        let Some(close) = rest[open + 1..].find('`') else {
            break;
        };
        html.push_str(&escape(&rest[..open]));
        html.push_str(&format!(
            "<code>{}</code>",
            escape(&rest[open + 1..open + 1 + close])
        ));
        rest = &rest[open + 1 + close + 1..];
    }
    html.push_str(&escape(rest));
    html
}

fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::{escape, first_paragraph, inline, link, markdown};

    #[test]
    fn links_are_relative_to_the_page() {
        assert_eq!(link("index", "app/orders.html"), "app/orders.html");
        assert_eq!(link("app/orders", "index.html"), "../index.html");
        assert_eq!(link("std/net/tls", "index.html"), "../../index.html");
    }

    #[test]
    fn text_is_escaped_everywhere() {
        assert_eq!(
            escape("a < b && \"c\" > d"),
            "a &lt; b &amp;&amp; &quot;c&quot; &gt; d"
        );
        assert_eq!(
            inline("use `List<T>` & <b>"),
            "use <code>List&lt;T&gt;</code> &amp; &lt;b&gt;"
        );
        assert_eq!(inline("an ` alone"), "an ` alone");
    }

    #[test]
    fn doc_comments_read_as_simple_markdown() {
        let text = "First line\nsame paragraph.\n\n- one\n  continued\n- `two`\nAfter.\n\n```chimi\nlet x = <1>\n```\n";
        assert_eq!(
            markdown(text),
            "<p>First line same paragraph.</p>\n\
             <ul>\n<li>one continued</li>\n<li><code>two</code></li>\n</ul>\n\
             <p>After.</p>\n\
             <pre><code>let x = &lt;1&gt;</code></pre>\n"
        );
    }

    #[test]
    fn blank_lines_separate_paragraphs() {
        assert_eq!(markdown("One.\n\nTwo."), "<p>One.</p>\n<p>Two.</p>\n");
    }

    #[test]
    fn a_fence_closes_only_at_one_as_long() {
        assert_eq!(
            markdown("````\n```\nstill code\n```chimi\n````"),
            "<pre><code>```\nstill code\n```chimi</code></pre>\n"
        );
    }

    #[test]
    fn the_index_shows_the_first_paragraph() {
        assert_eq!(
            first_paragraph("Opening a socket needs the `network`\nneed.\n\nMore."),
            Some("Opening a socket needs the `network` need.".to_owned())
        );
        assert_eq!(first_paragraph("```chimi\nlet x = 1\n```"), None);
    }

    #[test]
    fn an_unclosed_fence_runs_to_the_end() {
        assert_eq!(
            markdown("Text\n```\ncode\nmore"),
            "<p>Text</p>\n<pre><code>code\nmore</code></pre>\n"
        );
    }
}
