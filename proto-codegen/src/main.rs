//! `proto-codegen`: generates `src/generated/` of `mcbrs-protocol` from the
//! JSON schemas of <https://github.com/Mojang/bedrock-protocol-docs>.
//!
//! ```text
//! cargo run --bin proto-codegen [-- [--input <dir>] [--output <dir>] [--check]]
//! ```
//!
//! Pipeline: [`schema`] (strict parsing) -> [`lower`] (schema to [`ir`]) ->
//! [`cycles`] (`Box` insertion) -> [`emit`] (Rust source).

#![forbid(unsafe_code)]

mod cycles;
mod emit;
mod ir;
mod lower;
mod names;
mod overrides;
mod schema;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// `println!` panics when the stream is closed (`proto-codegen | head`), which
/// would abort the run halfway through writing files. Progress messages are
/// not worth failing for: write errors are ignored.
macro_rules! say {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stdout(), $($arg)*);
    }};
}

/// Same as [`say!`], on the error stream.
macro_rules! warn {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stderr(), $($arg)*);
    }};
}

struct Args {
    input: PathBuf,
    output: PathBuf,
    /// Verify that the files on disk are up to date instead of writing them.
    check: bool,
}

const USAGE: &str = "\
Usage: proto-codegen [--input <dir>] [--output <dir>] [--check]

  --input <dir>   directory of Mojang's JSON schemas
                  (default: <repo>/vendor/bedrock-protocol-docs/json)
  --output <dir>  directory receiving the generated Rust modules
                  (default: <repo>/src/generated)
  --check         do not write anything; fail if the output is out of date";

fn parse_args() -> Result<Option<Args>, String> {
    // The crate lives in <repo>/proto-codegen.
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().map(Path::to_path_buf).unwrap_or_default();
    let mut args = Args {
        input: repo.join("vendor").join("bedrock-protocol-docs").join("json"),
        output: repo.join("src").join("generated"),
        check: false,
    };
    let mut argv = std::env::args().skip(1);
    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "--input" => args.input = argv.next().ok_or("--input needs a directory")?.into(),
            "--output" => args.output = argv.next().ok_or("--output needs a directory")?.into(),
            "--check" => args.check = true,
            "-h" | "--help" => return Ok(None),
            other => return Err(format!("unknown argument {other:?}\n\n{USAGE}")),
        }
    }
    Ok(Some(args))
}

fn run(args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    let documents = schema::load_dir(&args.input)?;
    let mut model = lower::Lowerer::new(&documents).run()?;
    let boxed = cycles::break_cycles(&mut model);

    // Recorded by scripts/fetch-mojang-protocol.sh next to the vendored schemas.
    let commit = args
        .input
        .ancestors()
        .filter_map(|dir| std::fs::read_to_string(dir.join(".mojang-protocol-sha")).ok())
        .map(|sha| sha.trim().to_owned())
        .find(|sha| !sha.is_empty());

    let files = emit::Emitter::new(&model, commit).emit()?;

    say!(
        "{} schemas (Minecraft {}, protocol {}): {} packets, {} structs, {} variants, {} enums, {} aliases, {} boxed field(s)",
        documents.len(),
        model.minecraft_version,
        model.protocol_version,
        model.packets.len(),
        model.structs.len(),
        model.variants.len(),
        model.enums.len(),
        model.aliases.len(),
        boxed
    );

    if args.check {
        let stale: Vec<&str> = files
            .iter()
            .filter(|file| {
                let on_disk = std::fs::read_to_string(args.output.join(file.name)).unwrap_or_default();
                // Tolerate checkouts that converted line endings.
                on_disk.replace("\r\n", "\n") != file.contents
            })
            .map(|file| file.name)
            .collect();
        if stale.is_empty() {
            say!("{} is up to date", args.output.display());
            return Ok(());
        }
        return Err(format!("out of date: {} (run `cargo run --bin proto-codegen`)", stale.join(", ")).into());
    }

    std::fs::create_dir_all(&args.output)?;
    // Drop modules left over from a previous layout. Only files this tool
    // wrote are ever deleted, so a wrong `--output` cannot destroy sources.
    for entry in std::fs::read_dir(&args.output)? {
        let path = entry?.path();
        let is_rust = path.extension().and_then(|e| e.to_str()) == Some("rs");
        let known = path.file_name().and_then(|n| n.to_str()).is_some_and(|n| files.iter().any(|f| f.name == n));
        if is_rust && !known {
            let generated = std::fs::read_to_string(&path).is_ok_and(|text| text.starts_with(emit::GENERATED_MARKER));
            if generated {
                std::fs::remove_file(&path)?;
                say!("removed stale {}", path.display());
            } else {
                warn!("warning: leaving {} alone (not generated by proto-codegen)", path.display());
            }
        }
    }
    for file in &files {
        let path = args.output.join(file.name);
        // Leave untouched files alone so incremental builds stay fast.
        if std::fs::read_to_string(&path).ok().as_deref() != Some(file.contents.as_str()) {
            std::fs::write(&path, &file.contents)?;
        }
        say!("wrote {} ({} lines)", path.display(), file.contents.lines().count());
    }
    Ok(())
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(Some(args)) => args,
        Ok(None) => {
            say!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            warn!("error: {message}");
            return ExitCode::from(2);
        }
    };
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            warn!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
