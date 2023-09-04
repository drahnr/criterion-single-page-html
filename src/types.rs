use super::*;

#[derive(Debug, clap::Parser)]
pub(crate) struct CliArgs {
    #[arg(long)]
    pub(crate) root: PathBuf,

    #[clap(long, env = "DEST")]
    pub(crate) dest: PathBuf,
}

pub type DigestVal = GenericArray<u8, U32>;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PageWrapping {
    pub title: String,
    pub content: String,
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
        serializer.collect_str(hex::encode(self.digest).as_str())
    }
}

impl std::fmt::Display for PageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(self.digest))
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RenderItem {
    pub(crate) linkmarker: PageId,
    pub(crate) page: PageWrapping,
}

#[derive(askama::Template)]
#[template(path = "template.html", escape = "none")]
pub(crate) struct Temple {
    /// First page is the root.
    pub(crate) pages: Vec<RenderItem>,
}
