use super::*;

/// Command line arguments.
#[derive(Debug, clap::Parser)]
pub(crate) struct CliArgs {
    #[arg(long)]
    pub(crate) root: PathBuf,

    #[clap(long, env = "DEST")]
    pub(crate) dest: PathBuf,
}

/// Digest value, as produced by sha256
pub type DigestVal = GenericArray<u8, U32>;

/// Describes a page to be rendered
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PageWrapping {
    pub title: String,
    pub content: String,
}

/// A unique page identifier
///
/// Derived from the original data _before_ adjusting it.
/// It will be used as reference to the sections and assuring pages are only included once
/// if accessed via different links and relative URL paths.
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
    /// Derive the page id from the originally link content of the page.
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
        serializer.collect_str(hex::encode(self.digest).as_str())
    }
}

impl std::fmt::Display for PageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(self.digest))
    }
}

/// A page to render with the correct linkmarker to be used.
#[derive(Debug, Clone)]
pub(crate) struct RenderItem {
    pub(crate) linkmarker: PageId,
    pub(crate) page: PageWrapping,
}

/// Template baseline for the generated code.
#[derive(askama::Template)]
#[template(path = "template.html", escape = "none")]
pub(crate) struct Temple {
    /// First page is the root.
    pub(crate) pages: Vec<RenderItem>,
}
