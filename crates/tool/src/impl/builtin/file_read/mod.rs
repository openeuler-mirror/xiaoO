pub mod constants;
pub mod dedup;
pub mod device;
mod discovered_tool;
pub mod executor;
pub mod input;
pub mod output;
mod readers;
pub mod spec;
pub mod tokenizer;
pub mod validation;

pub use constants::PDF_MAX_PAGES_PER_READ;
pub use dedup::{get_file_mtime, DedupStateStore, FileReadState};
pub use device::is_blocked_device_path;
pub(crate) use discovered_tool::discover_file_read;
pub use executor::FileReadExecutor;
pub use input::FileReadInput;
pub use output::{
    FileReadOutput, FileUnchangedOutput, ImageDimensions, ImageOutput, NotebookCell,
    NotebookOutput, OutputContract, PartsOutput, PdfOutput, TextOutput,
};
pub use spec::FileReadToolSpec;
pub use tokenizer::estimate_tokens;
pub use validation::error_code;
pub use validation::{validate_input, ValidationResult};
