use askama::Template;
use clap::Parser;
use color_eyre::eyre::Result;
use fs_err as fs;

use html5ever::tendril::{StrTendril, TendrilSink};
use html5ever::tree_builder::QuirksMode;
use html5ever::{Attribute, ParseOpts};
use indexmap::IndexSet;
use markup5ever_rcdom::{Node, NodeData, RcDom};
use sha2::digest::generic_array::GenericArray;
use sha2::digest::typenum::U32;
use sha2::digest::Update;
use sha2::Digest;
use sha2::Sha256;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use std::path::PathBuf;

mod types;
use crate::types::*;

/// Create a data url with the given charset.
pub fn create_data_url(media_type: &str, charset: &str, data: &[u8]) -> String {
    let charset = charset.trim();
    let c: String = if !charset.is_empty() && !charset.eq_ignore_ascii_case("US-ASCII") {
        format!(";charset={}", charset)
    } else {
        "".to_string()
    };
    let encoder = base64::engine::general_purpose::STANDARD_NO_PAD;
    let encoded = base64::engine::Engine::encode(&encoder, data);
    format!("data:{}{};base64,{}", media_type, c, encoded)
}

/// Render the tree given by the `Node` instance.
pub fn node_to_content(node: std::rc::Rc<Node>) -> Result<String> {
    let mut dest = Vec::with_capacity(1024);
    let handle: markup5ever_rcdom::SerializableHandle = node.into();

    html5ever::serialize(&mut dest, &handle, Default::default())?;

    // pick the reduced content
    let content = String::from_utf8(dest)?;
    Ok(content)
}

pub fn load_string_from_disk(
    search_context: &std::path::Path,
    value: &StrTendril,
) -> Result<String> {
    let value = String::from(value);
    let p = search_context.join(&value);
    log::debug!("Loading {}  +  {} ", search_context.display(), value);
    let s = fs::read_to_string(p)?;
    Ok(s)
}

pub fn extract_html_title(base: &Rc<Node>) -> Option<String> {
    let mut maybe_title = None;
    extract_xml_node(base, |name: &str, nd: &Rc<Node>| {
        if let "title" = name {
            match nd.children.borrow().iter().next().map(|node| &node.data) {
                Some(NodeData::Text { contents }) => {
                    let cb = dbg!(contents.borrow());
                    maybe_title.replace(cb.to_string());
                    return Act::Break;
                }
                _x => {}
            }
        }
        Act::Next
    });
    maybe_title
}

pub fn extract_svg_title(base: &Rc<Node>) -> Option<String> {
    let mut set = IndexSet::new();
    extract_xml_node(base, |name: &str, nd: &Rc<Node>| {
        if let "title" | "text" = name {
            // in svgs all relevant things contain a `<tspan>` inner as the only inner element.
            let maybe_inner = nd.children.borrow();
            let maybe_inner = maybe_inner.iter().next();
            if let Some(inner) = maybe_inner.cloned() {
                let maybe_inner_data = inner.children.borrow();
                let maybe_inner_data_first =
                    maybe_inner_data.iter().next().map(|inner| &inner.data);
                match maybe_inner_data_first {
                    Some(NodeData::Text { contents }) => {
                        let cb = contents.borrow();
                        let _ = set.insert(dbg!(cb.to_string()));
                        return Act::Next;
                    }
                    _x => {}
                }
            }
        }
        Act::Next
    });
    set.retain(|x| !x.starts_with("Point estimate") && !x.starts_with("gnuplot_"));
    // specific to criterion, the last one is the actual name
    set.first().cloned()
}

pub fn extract_body(base: &Rc<Node>) -> Option<Rc<Node>> {
    let mut maybe_body = None;
    extract_xml_node(base, |name: &str, nd: &Rc<Node>| {
        if let "body" = name {
            maybe_body.replace(nd.clone());
            return Act::Break;
        }
        Act::Next
    });
    maybe_body
}

/// Iteration action to do, based on keywords or to process the `next` iteration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    Break,
    Continue,
    Next,
}

pub fn extract_xml_node(base: &Rc<Node>, mut fx: impl FnMut(&'_ str, &Rc<Node>) -> Act) {
    let mut queue = VecDeque::<Rc<Node>>::new();
    queue.push_back(base.clone());

    'search: while let Some(node) = queue.pop_back() {
        // use depth first, otherwise we won't find a title.
        match &node.data {
            NodeData::Element { ref name, .. } => {
                let local = name.local.to_string();
                match fx(local.as_str(), &node) {
                    Act::Break => break 'search,
                    Act::Continue => continue 'search,
                    Act::Next => {}
                }
            }
            _ => {}
        }
        queue.extend(node.children.clone().into_inner().into_iter());
    }
}

/// Parse the page and replace.
fn process_html_page(
    rootbase: &std::path::Path,
    current_html_file: &std::path::Path,
    parent_search_context: &std::path::Path,
    pages: &mut HashMap<PageId, PageWrapping>,
) -> Result<PageWrapping> {
    // let relative_to_root = parent_search_context.join(current_html_file);
    log::trace!(
        "Updating search context >{}< + >{}< ?",
        parent_search_context.display(),
        current_html_file.display()
    );
    let search_context = if let Some(current_base) = current_html_file.parent() {
        current_base.to_path_buf()
    } else {
        parent_search_context.to_path_buf()
    };
    let search_context = &search_context;

    log::debug!(
        "Processing file with context: search=\"{}\" ; {}",
        search_context.display(),
        current_html_file.display()
    );

    let mut opts = ParseOpts::default();
    opts.tokenizer.discard_bom = true;
    opts.tokenizer.exact_errors = true;
    opts.tree_builder.drop_doctype = true;
    opts.tree_builder.exact_errors = true;
    opts.tree_builder.quirks_mode = QuirksMode::NoQuirks;
    opts.tree_builder.scripting_enabled = false;

    let content = fs::read_to_string(current_html_file)?;
    let mut cursor = std::io::Cursor::new(content);
    let sink = RcDom::default();
    let doc: RcDom = html5ever::parse_document::<RcDom>(sink, opts.clone())
        .from_utf8()
        .read_from(&mut cursor)?;

    let maybe_title = extract_html_title(&doc.document);

    let title = if let Some(title) = maybe_title {
        log::debug!("Found title: \"{}\"", &title);
        title
    } else {
        log::warn!("Didn't find a title in \"{}\"", current_html_file.display());
        "Missing title".to_owned()
    };

    let maybe_body = extract_body(&doc.document);

    let Some(body_node) = maybe_body else {
        color_eyre::eyre::bail!(
            "Couldn't find <body> in the file: {}",
            current_html_file.display()
        );
    };

    let section_root_node = body_node.clone();
    let mut queue = VecDeque::<Rc<Node>>::new();
    queue.push_back(body_node);
    assert_eq!(queue.len(), 1);

    while let Some(node) = queue.pop_back() {
        match &node.data {
            NodeData::Document => unreachable!(),

            NodeData::Text { contents: _ } => {}

            NodeData::Element {
                ref name,
                ref attrs,
                template_contents,
                ..
            } => {
                if let Some(inner) = template_contents.borrow().clone() {
                    queue.push_back(inner.clone());
                    log::trace!("Add template inner contents");
                }

                queue.extend(node.children.clone().into_inner().into_iter());

                // get src
                if let Some(Attribute {
                    name: _attr_name,
                    value: ref mut uri,
                }) = attrs
                    .borrow_mut()
                    .iter_mut()
                    .find(|Attribute { name, value: _ }| name.local.as_ref() == "src")
                {
                    // maintain only local links, anything that is point to some http service will be ignored
                    if uri.starts_with("http") {
                        log::debug!(
                            "Ignoring http prefix'd <{} src=\"..\" ..>",
                            name.local.as_ref()
                        );
                        continue;
                    }

                    match name.local.as_ref() {
                        "img" => match load_string_from_disk(search_context, uri) {
                            Ok(data) => {
                                let media_type = if uri.ends_with("svg") {
                                    "image/svg+xml"
                                } else if uri.ends_with("png") {
                                    "image/png"
                                } else {
                                    ""
                                };
                                let url_content_bas64 =
                                    create_data_url(media_type, "UTF-8", data.as_bytes());
                                *uri = url_content_bas64.as_str().into();
                            }
                            Err(err) => {
                                log::warn!("Couldn't find referenced file, ignoring: {err:?}");
                            }
                        },
                        _name => {}
                    }
                }

                // get href
                if let Some(Attribute {
                    name: _attr_name,
                    value: ref mut uri,
                }) = attrs
                    .borrow_mut()
                    .iter_mut()
                    .find(|Attribute { name, value: _ }| name.local.as_ref() == "href")
                {
                    // maintain only local links, anything that is point to some http service will be ignored
                    if uri.starts_with("http") {
                        log::debug!(
                            "Ignoring http prefix'd <{} link=\"..\" ..>",
                            name.local.as_ref()
                        );
                        continue;
                    }

                    match name.local.as_ref() {
                        "link" => match load_string_from_disk(search_context, uri) {
                            Ok(data) => {
                                let url_content_bas64 =
                                    create_data_url("", "UTF-8", data.as_bytes());

                                *uri = url_content_bas64.as_str().into();

                                let uri = PathBuf::from(String::from(&*uri));
                                log::info!(
                                    "Loading <link href=\"{}\"> as data with search-ctx=\"{}\"",
                                    uri.display(),
                                    search_context.display()
                                );
                            }
                            Err(err) => {
                                log::warn!("Couldn't find referenced file, ignoring: {err:?}");
                            }
                        },
                        "a" => {
                            let child_linked = PathBuf::from(String::from(&*uri));

                            match load_string_from_disk(search_context, uri) {
                                Err(err) => {
                                    log::warn!("Couldn't find referenced file, ignoring: {err:?}");
                                }
                                Ok(target_xml_page_content) => {
                                    log::info!("Found outgoing link <a href=\"{}\"> as data with search-ctx=\"{}\"", child_linked.display(), search_context.display());

                                    let linked_page_id =
                                        PageId::from_content(&target_xml_page_content);

                                    *uri = format!("#{}", &linked_page_id).as_str().into();

                                    // svgs can be linked, but we want to inline them as separate pages without traversal, for now we create a data link
                                    if let Some("svg") =
                                        child_linked.extension().and_then(|x| x.to_str())
                                    {
                                        let maybe_title = {
                                            let mut cursor = std::io::Cursor::new(
                                                target_xml_page_content.as_str(),
                                            );
                                            let sink = RcDom::default();
                                            let doc: RcDom = html5ever::parse_document::<RcDom>(
                                                sink,
                                                opts.clone(),
                                            )
                                            .from_utf8()
                                            .read_from(&mut cursor)?;

                                            extract_svg_title(&doc.document)
                                        };
                                        pages.insert(
                                            linked_page_id,
                                            PageWrapping {
                                                title: maybe_title.unwrap_or("Unknown".to_owned()),
                                                content: target_xml_page_content,
                                            },
                                        );
                                        continue;
                                    }

                                    if !pages.contains_key(&linked_page_id) {
                                        log::debug!("Found a new page! rootbase=\"{}\" current=\"{}\" search-ctx=\"{}\"",
                                            rootbase.display(),
                                            child_linked.display(),
                                            search_context.display()
                                        );
                                        let modified_page = process_html_page(
                                            rootbase,
                                            search_context
                                                .as_path()
                                                .join(child_linked.as_path())
                                                .as_path(),
                                            search_context,
                                            pages,
                                        )?;
                                        pages.insert(linked_page_id, modified_page);
                                    } else {
                                        log::debug!(
                                            "Page already processed {}",
                                            child_linked.display()
                                        );
                                    }
                                }
                            }
                        }
                        _ => continue,
                    };
                } else {
                    // no a link, don't touch
                }
            }

            _y => {
                unreachable!("{:?}", _y);
            }
        }
    }

    let modified_page_content = node_to_content(section_root_node)?;
    Ok(PageWrapping {
        content: modified_page_content,
        title: title.to_owned(),
    })
}

fn main() -> Result<()> {
    pretty_env_logger::formatted_builder()
        .format_level(true)
        .filter_level(log::LevelFilter::Trace)
        .filter_module("html5ever", log::LevelFilter::Off)
        .filter_module("criterion-single-page-html", log::LevelFilter::Trace)
        .filter_module("self", log::LevelFilter::Trace)
        .filter_module("", log::LevelFilter::Trace)
        .filter_module("::", log::LevelFilter::Trace)
        .try_init()?;
    let CliArgs { root, dest, .. } = CliArgs::parse();

    let mut pages = HashMap::new();
    let _content = fs::read_to_string(&root)?;

    let _cwd = std::env::current_dir()?;
    let x = root.parent().unwrap();
    process_html_page(x, &root, x, &mut pages)?;

    let pages = Vec::from_iter(pages.into_iter().map(|(digest, page)| {
        let page_id = digest.clone();
        RenderItem {
            linkmarker: page_id.clone(),
            page,
        }
    }));

    let t = Temple { pages };

    let single_page = t.render()?;

    fs_err::write(dest, single_page.as_bytes())?;

    Ok(())
}
