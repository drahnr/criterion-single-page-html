use clap::Parser;
use html5ever::{ParseOpts, Attribute};
use html5ever::tree_builder::QuirksMode;
use html5ever::tendril::TendrilSink;
use markup5ever_rcdom::{RcDom, NodeData, Node};
use sha2::Sha256;
use sha2::Digest;
use sha2::digest::generic_array::{GenericArray, ArrayLength};
use sha2::digest::typenum::U32;
use std::collections::hash_map::{VacantEntry, Entry};
use std::path::PathBuf;
use std::io::{BufRead,BufReader, Seek, SeekFrom, Read};
use std::collections::{HashSet, HashMap, VecDeque};
use askama::Template;

use color_eyre::eyre::Result;

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
    let c: String =
        if !charset.is_empty() && !charset.eq_ignore_ascii_case("US-ASCII") {
            format!(";charset={}", charset)
        } else {
            "".to_string()
        };
    format!("data:,{}{};base64,{}", media_type, c, base64::encode(data))
}

#[derive(Debug, Clone)]
struct ReplaceMe {
    content: String,
    rmt: ReplaceMeType,
}


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReplaceMeType {
    Link,
    A,
}

pub type DigestVal = GenericArray<u8, U32>;

/// Parse the page and replace
fn parse_page<Y>(mut io: Y, pages: &mut HashMap<DigestVal, String>) -> Result<()> where Y: BufRead + Seek {
    eprintln!(">>> ");
    
    let mut opts = ParseOpts::default(); 
    opts.tokenizer.discard_bom = true;
    opts.tokenizer.exact_errors = true;
    opts.tree_builder.drop_doctype = true;
    opts.tree_builder.exact_errors = true;
    opts.tree_builder.quirks_mode = QuirksMode::NoQuirks;
    opts.tree_builder.scripting_enabled = false;
    
    
    let mut sink = RcDom::default();
    let doc : RcDom = html5ever::parse_document::<RcDom>(sink, opts)
        .from_utf8()
        .read_from(&mut io)?;
    
    let mut queue : VecDeque<_> = VecDeque::from_iter(doc.document.children.clone().into_inner().into_iter());
    
    while let Some(node) = queue.pop_front() {
        match &node.data {
            NodeData::Document => unreachable!("xxx"),

            NodeData::Text { ref contents } => {}

            NodeData::Element { ref name, ref attrs, template_contents, .. } => {
                eprintln!(">>> {name:?}");

                if let Some(inner) = template_contents.borrow().clone() {
                    queue.push_back(inner.clone());
                    eprintln!("Add template inner contents");
                }
                
                queue.extend(node.children.clone().into_inner().into_iter());
                
                // get href
                if let Some(Attribute { name: attr_name, value }) = attrs.borrow().iter().find(|Attribute { name, value }| name.local.as_ref() == "href" ) {
                    let rmt = match dbg!(name.local.as_ref()) {
                        "link" => ReplaceMeType::Link,
                        "a" => ReplaceMeType::A,
                        _ => {
                            eprintln!("Not fish not meat");
                            continue
                        },
                    };
                    // maintain only local links, anything that is point to some http service will be ignored
                    if value.starts_with("http") {
                        eprintln!("Ignoreing http prefix'd link");
                        continue;
                    }
                    
                    let mut hasher = Sha256::new();
                    std::io::copy(&mut io, &mut hasher)?;
                    let k = hasher.finalize();

                    let mut content = BufReader::new(fs_err::File::open( value.to_string() )?);
                    
                    match pages.entry(k) {
                        Entry::Vacant(vacant) => {
                            
                            eprintln!("First time");
                            content.seek(SeekFrom::Start(0));
                            let mut v = vec![];
                            content.read_to_end(&mut v)?;
                            {
                                let content = String::from_utf8_lossy(&v);
                                let content = content.to_string();
                                vacant.insert(content);
                                
                                eprintln!("**** INSERTING PAGE ****");
                            }
                            // follow the links
                            eprintln!("Following link");

                            parse_page(content, pages)?;
                        }
                        _ => {}
                    }         
                } else {
                    eprintln!("No href present, skipping.");
                    // no a link, don't touch
                }
            },

            _y => {
              unreachable!("{:?}", _y);  
            },
        }

    }
    
    Ok(())
}

fn foo(digest: DigestVal) -> String {
    format!("{:?}", digest.as_slice())
}


#[derive(Debug, Clone)]
struct X {
    link: String,
    title: String,
    content: String,
}

#[derive(askama::Template)]
#[template(path="template.html")]
struct Temple {
    /// First page is the root.
    pages: Vec<X>,
}

fn main() -> Result<()> {
    let CliArgs { root, dest, .. } = CliArgs::parse();
    
    let mut pages = HashMap::new();
    let content = BufReader::new(fs_err::File::open(root)?);
    parse_page(content, &mut pages)?;
    let pages = Vec::from_iter(pages.into_iter().map(|(digest, content)| { 
        X {
            link: foo(digest.clone()),
            title: foo(digest),
            content,
        }
    }));
    
    let t = Temple { pages };
    
    let single_page = t.render()?;
    
    fs_err::write(dest, single_page.as_bytes())?;
    
    Ok(())
}
