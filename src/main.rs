use askama::Template;
use clap::Parser;
use color_eyre::eyre::Result;
use fs_err as fs;
use html5ever::serialize::HtmlSerializer;
use html5ever::tendril::{StrTendril, TendrilSink};
use html5ever::tree_builder::QuirksMode;
use html5ever::{Attribute, ParseOpts};
use markup5ever_rcdom::{Node, NodeData, RcDom};
use sha2::digest::generic_array::{ArrayLength, GenericArray};
use sha2::digest::typenum::U32;
use sha2::digest::Update;
use sha2::Digest;
use sha2::Sha256;
use std::collections::hash_map::{Entry, VacantEntry};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::thread::current;

#[derive(Debug, clap::Parser)]
struct CliArgs {
    #[arg(long)]
    root: PathBuf,

    #[clap(long, env = "DEST")]
    dest: PathBuf,
}

/// Create a data url with the given charset.
pub fn create_data_url(media_type: &str, charset: &str, data: &[u8]) -> String {
    let charset = charset.trim();
    let c: String = if !charset.is_empty() && !charset.eq_ignore_ascii_case("US-ASCII") {
        format!(";charset={}", charset)
    } else {
        "".to_string()
    };
    format!("data:{}{};base64,{}", media_type, c, base64::encode(data))
}

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
    let value = String::from(&*value);
    let p = search_context.join(&value);
    log::info!("Loading {}  /^\\  {} ", search_context.display(), value);
    let s = fs::read_to_string(p)?;
    Ok(s)
}

pub type DigestVal = GenericArray<u8, U32>;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PageWrapping {
    title: String,
    content: String,
}

/// Parse the page and replace.
fn process_html_page(
    rootbase: &std::path::Path,
    current_html_file: &std::path::Path,
    parent_search_context: &std::path::Path,
    pages: &mut HashMap<PageId, PageWrapping>,
) -> Result<PageWrapping> {
    let search_context = if let Some(current_base) = current_html_file.parent() {
        if current_base.is_relative() {
            parent_search_context.join(current_base)
        } else {
            parent_search_context.to_path_buf()
        }
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

    let content = fs::read_to_string(&current_html_file)?;
    let mut cursor = std::io::Cursor::new(content);
    let mut sink = RcDom::default();
    let doc: RcDom = html5ever::parse_document::<RcDom>(sink, opts)
        .from_utf8()
        .read_from(&mut cursor)?;

    let mut maybe_title = None;

    let mut queue = VecDeque::from_iter(doc.document.children.clone().into_inner().into_iter());
    'search: while let Some(node) = queue.pop_back() {
        // use depth first, otherwise we won't find a title.
        match &node.data {
            NodeData::Element {
                ref name,
                ref template_contents,
                ..
            } => {
                let local = name.local.to_string();
                if local == "title" {
                    if let Some(ref x) = *template_contents.borrow() {
                        if let NodeData::Text {
                            contents: title_content,
                        } = dbg!(&x.as_ref().data)
                        {
                            log::debug!("Found <title> in file: {}", current_html_file.display());
                            let title = &*title_content.borrow();
                            maybe_title.replace(title.into());
                        }
                    } else {
                        log::error!("Found title, but wasn't a simple one")
                    }
                }
                if local == "body" {
                    log::debug!("Found <body> in file: {}", current_html_file.display());
                    queue.clear();
                    queue.push_back(node);
                    break 'search;
                }
                queue.extend(node.children.clone().into_inner().into_iter());
            }
            _ => {}
        }
    }

    let Some(node) = queue.pop_back() else {
        color_eyre::eyre::bail!(
            "Couldn't find <body> in the file: {}",
            current_html_file.display()
        );
    };
    let content = node_to_content(node.clone())?;
    let content = content.as_str();

    let section_root_node = node.clone();
    queue.push_back(node);
    assert_eq!(queue.len(), 1);

    let title = maybe_title
        .map(|title: String| title.rsplit_once("-").map(|(a, _b)| a.to_owned()))
        .flatten()
        .unwrap_or("???".to_owned());

    while let Some(node) = queue.pop_back() {
        match &node.data {
            NodeData::Document => unreachable!(),

            NodeData::Text { ref contents } => {}

            NodeData::Element {
                ref name,
                ref attrs,
                template_contents,
                ..
            } => {
                if let Some(inner) = template_contents.borrow().clone() {
                    queue.push_back(inner.clone());
                    log::info!("Add template inner contents");
                }

                queue.extend(node.children.clone().into_inner().into_iter());

                // get src
                if let Some(Attribute {
                    name: attr_name,
                    value: ref mut uri,
                }) = attrs
                    .borrow_mut()
                    .iter_mut()
                    .find(|Attribute { name, value }| name.local.as_ref() == "src")
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
                        "img" => match load_string_from_disk(&search_context, uri) {
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
                    name: attr_name,
                    value: ref mut uri,
                }) = attrs
                    .borrow_mut()
                    .iter_mut()
                    .find(|Attribute { name, value }| name.local.as_ref() == "href")
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

                                    /// svgs can be linked, but we want to inline them as separate pages without traversal, for now we create a data link
                                    if let Some("svg") =
                                        child_linked.extension().map(|x| x.to_str()).flatten()
                                    {
                                        pages.insert(
                                            linked_page_id,
                                            PageWrapping {
                                                title: "I am a svg".to_owned(),
                                                content: target_xml_page_content,
                                            },
                                        );
                                        continue;
                                    }

                                    if !pages.contains_key(&linked_page_id) {
                                        log::debug!("Found a new page!");

                                        log::warn!("Processing page rootbase=\"{}\" current=\"{}\" search-ctx=\"{}\"",
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
                                            &search_context,
                                            pages,
                                        )?;
                                        pages.insert(linked_page_id, modified_page);
                                    } else {
                                        log::trace!(
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

fn foo(digest: DigestVal) -> String {
    format!("{:?}", digest.as_slice())
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct PageId {
    digest: DigestVal,
}

impl From<DigestVal> for PageId {
    fn from(digest: DigestVal) -> Self {
        Self { digest }
    }
}

impl PageId {
    pub(crate) fn from_content(s: &str) -> Self {
        Self {
            digest: Sha256::new().chain(s).finalize(),
        }
    }
}

impl serde::ser::Serialize for PageId {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_str(hex::encode(&self.digest).as_str())
    }
}

impl std::fmt::Display for PageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(&self.digest))
    }
}

#[derive(Debug, Clone)]
struct RenderItem {
    linkmarker: PageId,
    page: PageWrapping,
}

#[derive(askama::Template)]
#[template(path = "template.html", escape = "none")]
struct Temple {
    /// First page is the root.
    pages: Vec<RenderItem>,
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
    let content = fs::read_to_string(&root)?;

    let cwd = std::env::current_dir()?;
    let x = root.parent().unwrap();
    process_html_page(&x, &root, &x, &mut pages)?;

    let pages = Vec::from_iter(pages.into_iter().map(|(digest, page)| {
        let page_id = PageId::from(digest.clone());
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
