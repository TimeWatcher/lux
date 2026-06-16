use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::process::ExitCode;

use luxc::codegen::LuaCodegen;
use luxc::compile_time::CompileTimePackageRegistry;
use luxc::diag::DiagnosticEmitter;
use luxc::format::format_source;
use luxc::host::HostRegistry;
use luxc::lex::Lexer;
use luxc::lint::{LintOptions, lint_module};
use luxc::lower::Lowerer;
use luxc::package_manager::{
    DependencySource, InitOptions, InstallRequest, LockRequest, RemoveRequest,
    doctor as package_doctor, init_project, install_package, list_locked, lock_project,
    remove_package,
};
use luxc::pipeline::parse_expand_resolve;
use luxc::project::{GmodBuildOptions, ProjectManifest, build_gmod_project};
use luxc::runtime_map::map_generated_line;
use luxc::source::SourceFile;
use luxc::sourcemap::{SourceCommentMode, map_after_source_comments, with_source_comments};

fn usage() {
    eprintln!("usage:");
    eprintln!("  luxc lex <path>");
    eprintln!("  luxc parse <path>");
    eprintln!("  luxc lint <path>");
    eprintln!("  luxc format <path> [--check] [--write]");
    eprintln!(
        "  luxc init [path] [--name <name>] [--std] [--out <path>] [--runtime-base <path>] [--no-autorun]"
    );
    eprintln!(
        "  luxc install <package-id> --from <github:owner/repo|url|path> [--tag <tag>|--branch <branch>|--commit <commit>]"
    );
    eprintln!("  luxc remove <package-id> [--project <project-root>]");
    eprintln!("  luxc lock [project-root]");
    eprintln!("  luxc list [project-root]");
    eprintln!("  luxc doctor [project-root]");
    eprintln!(
        "  luxc compile <path> [--map <path>] [--source-comments [none|readable|boundary|dense]]"
    );
    eprintln!("  luxc map-error <map.json> <generated-line>");
    eprintln!(
        "  luxc gmod build <source-root> --out <path> [--runtime-base <path>] [--no-autorun] [--dry-run]"
    );
    eprintln!(
        "  luxc gmod build --manifest <lux.toml> [--out <path>] [--runtime-base <path>] [--no-autorun] [--dry-run]"
    );
    eprintln!(
        "  luxc gmod package --manifest <lux.toml> --root <path> --gmad <path> --out <path> [--run] [--build-out <path>] [--runtime-base <path>] [--no-autorun]"
    );
    eprintln!(
        "  luxc gmod api update [--out <path>] [--coverage-out <path>] [--cache-dir <path>] [--offline] [--allow-failures]"
    );
}

fn lex_file(path: PathBuf) -> Result<ExitCode, String> {
    let file = SourceFile::load(0, &path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;

    let output = Lexer::new(&file).lex_all();

    for diagnostic in &output.diagnostics {
        eprintln!("{}", DiagnosticEmitter::render(diagnostic, &file));
    }

    if output.has_errors() {
        return Ok(ExitCode::from(1));
    }

    for token in output.tokens {
        let (line, col) = file.line_col(token.span.byte_start);
        println!("{line:>4}:{col:<4} {}", token.kind);
    }

    Ok(ExitCode::SUCCESS)
}

fn parse_file(path: PathBuf) -> Result<ExitCode, String> {
    let file = SourceFile::load(0, &path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;

    let lex = Lexer::new(&file).lex_all();

    for diagnostic in &lex.diagnostics {
        eprintln!("{}", DiagnosticEmitter::render(diagnostic, &file));
    }

    if lex.has_errors() {
        return Ok(ExitCode::from(1));
    }

    let parsed = parse_expand_resolve(&file, &lex.tokens);
    for diagnostic in &parsed.diagnostics {
        eprintln!("{}", DiagnosticEmitter::render(diagnostic, &file));
    }

    if parsed.has_errors() {
        return Ok(ExitCode::from(1));
    }

    println!("{:#?}", parsed.module);
    Ok(ExitCode::SUCCESS)
}

fn lint_file(path: PathBuf) -> Result<ExitCode, String> {
    let file = SourceFile::load(0, &path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;

    let lex = Lexer::new(&file).lex_all();
    for diagnostic in &lex.diagnostics {
        eprintln!("{}", DiagnosticEmitter::render(diagnostic, &file));
    }
    if lex.has_errors() {
        return Ok(ExitCode::from(1));
    }

    let parsed = parse_expand_resolve(&file, &lex.tokens);
    for diagnostic in &parsed.diagnostics {
        eprintln!("{}", DiagnosticEmitter::render(diagnostic, &file));
    }
    if parsed.has_errors() {
        return Ok(ExitCode::from(1));
    }

    let diagnostics = lint_module(&parsed.module, &file, LintOptions::default());
    for diagnostic in &diagnostics {
        eprintln!("{}", DiagnosticEmitter::render(diagnostic, &file));
    }

    Ok(ExitCode::SUCCESS)
}

fn format_file(path: PathBuf, check: bool, write: bool) -> Result<ExitCode, String> {
    let file = SourceFile::load(0, &path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let output = format_source(&file);
    for diagnostic in &output.diagnostics {
        eprintln!("{}", DiagnosticEmitter::render(diagnostic, &file));
    }
    if output
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == luxc::diag::Severity::Error)
    {
        return Ok(ExitCode::from(1));
    }

    if check {
        if output.text != file.text {
            eprintln!("{} is not formatted", path.display());
            return Ok(ExitCode::from(1));
        }
    } else if write {
        if output.text != file.text {
            write_file_atomically(&path, output.text.as_bytes())?;
            println!("formatted {}", path.display());
        }
    } else {
        print!("{}", output.text);
    }

    Ok(ExitCode::SUCCESS)
}

fn write_file_atomically(path: &Path, contents: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let file_name = path
        .file_name()
        .ok_or_else(|| format!("cannot format path without a file name: {}", path.display()))?
        .to_string_lossy();

    let mut attempt = 0u32;
    loop {
        let temp_path = parent.join(format!(".{file_name}.luxfmt-{attempt}.tmp"));
        let mut file = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
        {
            Ok(file) => file,
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists && attempt < 100 => {
                attempt += 1;
                continue;
            }
            Err(err) => {
                return Err(format!("failed to create {}: {err}", temp_path.display()));
            }
        };

        if let Err(err) = file.write_all(contents).and_then(|_| file.sync_all()) {
            let _ = fs::remove_file(&temp_path);
            return Err(format!("failed to write {}: {err}", temp_path.display()));
        }
        drop(file);

        if let Err(err) = replace_file(&temp_path, path) {
            let _ = fs::remove_file(&temp_path);
            return Err(format!("failed to replace {}: {err}", path.display()));
        }

        return Ok(());
    }
}

#[cfg(not(windows))]
fn replace_file(temp_path: &Path, path: &Path) -> std::io::Result<()> {
    fs::rename(temp_path, path)
}

#[cfg(windows)]
fn replace_file(temp_path: &Path, path: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(
            lp_existing_file_name: *const u16,
            lp_new_file_name: *const u16,
            dw_flags: u32,
        ) -> i32;
    }

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    let from = wide(temp_path);
    let to = wide(path);
    let ok = unsafe {
        MoveFileExW(
            from.as_ptr(),
            to.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if ok == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn compile_file(
    path: PathBuf,
    map_path: Option<PathBuf>,
    source_comments: SourceCommentMode,
) -> Result<ExitCode, String> {
    let file = SourceFile::load(0, &path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;

    let lex = Lexer::new(&file).lex_all();
    for diagnostic in &lex.diagnostics {
        eprintln!("{}", DiagnosticEmitter::render(diagnostic, &file));
    }
    if lex.has_errors() {
        return Ok(ExitCode::from(1));
    }

    let parsed = parse_expand_resolve(&file, &lex.tokens);
    for diagnostic in &parsed.diagnostics {
        eprintln!("{}", DiagnosticEmitter::render(diagnostic, &file));
    }
    if parsed.has_errors() {
        return Ok(ExitCode::from(1));
    }

    let ir = Lowerer::lower(&parsed.module, &parsed.resolved)
        .map_err(|err| format!("lowering failed: {err}"))?;
    let compile_time_registry =
        CompileTimePackageRegistry::load_default().map_err(|err| err.to_string())?;
    let host_registry = HostRegistry::from_specs(
        compile_time_registry
            .host_transform_specs()
            .map_err(|err| err.to_string())?,
    );
    let transformed = host_registry.transform_module(ir, &parsed.resolved);
    for diagnostic in &transformed.diagnostics {
        eprintln!("{}", DiagnosticEmitter::render(diagnostic, &file));
    }
    if transformed.has_errors() {
        return Ok(ExitCode::from(1));
    }
    let ir = transformed.module;

    match LuaCodegen::generate(&ir) {
        Ok(output) => {
            if let Some(map_path) = map_path {
                let source_map = if source_comments != SourceCommentMode::None {
                    map_after_source_comments(
                        &output.lua,
                        &output.source_map,
                        &file,
                        source_comments,
                    )
                } else {
                    output.source_map.clone()
                };
                let json = source_map.to_json(&[&file]);
                fs::write(&map_path, json)
                    .map_err(|err| format!("failed to write {}: {err}", map_path.display()))?;
            }
            if source_comments != SourceCommentMode::None {
                print!(
                    "{}",
                    with_source_comments(&output.lua, &output.source_map, &file, source_comments)
                );
            } else {
                print!("{}", output.lua);
            }
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => Err(format!("codegen failed: {err}")),
    }
}

fn main() -> ExitCode {
    let mut args = env::args_os();
    let _exe = args.next();

    let rest = args.collect::<Vec<_>>();

    match parse_command(rest) {
        Command::Lex(path) => match lex_file(path) {
            Ok(code) => code,
            Err(message) => {
                eprintln!("{message}");
                ExitCode::from(1)
            }
        },
        Command::Parse(path) => match parse_file(path) {
            Ok(code) => code,
            Err(message) => {
                eprintln!("{message}");
                ExitCode::from(1)
            }
        },
        Command::Lint(path) => match lint_file(path) {
            Ok(code) => code,
            Err(message) => {
                eprintln!("{message}");
                ExitCode::from(1)
            }
        },
        Command::Format { path, check, write } => match format_file(path, check, write) {
            Ok(code) => code,
            Err(message) => {
                eprintln!("{message}");
                ExitCode::from(1)
            }
        },
        Command::Compile {
            path,
            map_path,
            source_comments,
        } => match compile_file(path, map_path, source_comments) {
            Ok(code) => code,
            Err(message) => {
                eprintln!("{message}");
                ExitCode::from(1)
            }
        },
        Command::MapError {
            map_path,
            generated_line,
        } => match map_error(map_path, generated_line) {
            Ok(code) => code,
            Err(message) => {
                eprintln!("{message}");
                ExitCode::from(1)
            }
        },
        Command::Init(options) => match package_init(options) {
            Ok(code) => code,
            Err(message) => {
                eprintln!("{message}");
                ExitCode::from(1)
            }
        },
        Command::Install(request) => match package_install(request) {
            Ok(code) => code,
            Err(message) => {
                eprintln!("{message}");
                ExitCode::from(1)
            }
        },
        Command::Remove(request) => match package_remove(request) {
            Ok(code) => code,
            Err(message) => {
                eprintln!("{message}");
                ExitCode::from(1)
            }
        },
        Command::Lock { project_root } => match package_lock(project_root) {
            Ok(code) => code,
            Err(message) => {
                eprintln!("{message}");
                ExitCode::from(1)
            }
        },
        Command::List { project_root } => match package_list(project_root) {
            Ok(code) => code,
            Err(message) => {
                eprintln!("{message}");
                ExitCode::from(1)
            }
        },
        Command::Doctor { project_root } => match package_doctor_command(project_root) {
            Ok(code) => code,
            Err(message) => {
                eprintln!("{message}");
                ExitCode::from(1)
            }
        },
        Command::GmodBuild {
            manifest,
            source_root,
            output_root,
            runtime_base,
            autorun,
            dry_run,
        } => match gmod_build(
            manifest,
            source_root,
            output_root,
            runtime_base,
            autorun,
            dry_run,
        ) {
            Ok(code) => code,
            Err(message) => {
                eprintln!("{message}");
                ExitCode::from(1)
            }
        },
        Command::GmodPackage {
            manifest,
            package_root,
            output_root,
            runtime_base,
            autorun,
            gmad_path,
            output_gma,
            run,
        } => match gmod_package(
            manifest,
            package_root,
            output_root,
            runtime_base,
            autorun,
            gmad_path,
            output_gma,
            run,
        ) {
            Ok(code) => code,
            Err(message) => {
                eprintln!("{message}");
                ExitCode::from(1)
            }
        },
        Command::GmodApiUpdate { args } => match gmod_api_update(args) {
            Ok(code) => code,
            Err(message) => {
                eprintln!("{message}");
                ExitCode::from(1)
            }
        },
        Command::Invalid => {
            usage();
            ExitCode::from(2)
        }
    }
}

enum Command {
    Lex(PathBuf),
    Parse(PathBuf),
    Lint(PathBuf),
    Format {
        path: PathBuf,
        check: bool,
        write: bool,
    },
    Compile {
        path: PathBuf,
        map_path: Option<PathBuf>,
        source_comments: SourceCommentMode,
    },
    MapError {
        map_path: PathBuf,
        generated_line: usize,
    },
    Init(InitOptions),
    Install(InstallRequest),
    Remove(RemoveRequest),
    Lock {
        project_root: PathBuf,
    },
    List {
        project_root: PathBuf,
    },
    Doctor {
        project_root: PathBuf,
    },
    GmodBuild {
        manifest: Option<PathBuf>,
        source_root: Option<PathBuf>,
        output_root: Option<PathBuf>,
        runtime_base: Option<PathBuf>,
        autorun: Option<bool>,
        dry_run: bool,
    },
    GmodPackage {
        manifest: PathBuf,
        package_root: PathBuf,
        output_root: Option<PathBuf>,
        runtime_base: Option<PathBuf>,
        autorun: Option<bool>,
        gmad_path: PathBuf,
        output_gma: PathBuf,
        run: bool,
    },
    GmodApiUpdate {
        args: Vec<String>,
    },
    Invalid,
}

fn parse_command(args: Vec<OsString>) -> Command {
    match args.as_slice() {
        [command, path] if command == "lex" => Command::Lex(path.into()),
        [command, path] if command == "parse" => Command::Parse(path.into()),
        [command, path] if command == "lint" => Command::Lint(path.into()),
        [command, path, rest @ ..] if command == "format" => {
            parse_format_command(path.into(), rest)
        }
        [command, path, rest @ ..] if command == "compile" => {
            parse_compile_command(path.into(), rest)
        }
        [command, map_path, line] if command == "map-error" => {
            let Ok(generated_line) = line.to_string_lossy().parse::<usize>() else {
                return Command::Invalid;
            };
            Command::MapError {
                map_path: map_path.into(),
                generated_line,
            }
        }
        [command, rest @ ..] if command == "init" => parse_init_command(rest),
        [command, rest @ ..] if command == "install" => parse_install_command(rest),
        [command, rest @ ..] if command == "remove" => parse_remove_command(rest),
        [command] if command == "lock" => Command::Lock {
            project_root: PathBuf::from("."),
        },
        [command, project_root] if command == "lock" => Command::Lock {
            project_root: project_root.into(),
        },
        [command] if command == "list" => Command::List {
            project_root: PathBuf::from("."),
        },
        [command, project_root] if command == "list" => Command::List {
            project_root: project_root.into(),
        },
        [command] if command == "doctor" => Command::Doctor {
            project_root: PathBuf::from("."),
        },
        [command, project_root] if command == "doctor" => Command::Doctor {
            project_root: project_root.into(),
        },
        [scope, command, rest @ ..] if scope == "gmod" && command == "build" => {
            parse_gmod_build_command(rest)
        }
        [scope, command, rest @ ..] if scope == "gmod" && command == "package" => {
            parse_gmod_package_command(rest)
        }
        [scope, area, command, rest @ ..]
            if scope == "gmod" && area == "api" && command == "update" =>
        {
            parse_gmod_api_update_command(rest)
        }
        _ => Command::Invalid,
    }
}

fn parse_format_command(path: PathBuf, rest: &[OsString]) -> Command {
    let mut check = false;
    let mut write = false;
    for arg in rest {
        match arg.to_string_lossy().as_ref() {
            "--check" => check = true,
            "--write" => write = true,
            _ => return Command::Invalid,
        }
    }
    if check && write {
        return Command::Invalid;
    }
    Command::Format { path, check, write }
}

fn parse_compile_command(path: PathBuf, rest: &[OsString]) -> Command {
    let mut map_path = None;
    let mut source_comments = SourceCommentMode::None;
    let mut index = 0;

    while index < rest.len() {
        match rest[index].to_string_lossy().as_ref() {
            "--map" => {
                let Some(path) = rest.get(index + 1) else {
                    return Command::Invalid;
                };
                map_path = Some(PathBuf::from(path));
                index += 2;
            }
            "--source-comments" => {
                if let Some(value) = rest.get(index + 1).and_then(|arg| arg.to_str()) {
                    if let Some(mode) = SourceCommentMode::parse(value) {
                        source_comments = mode;
                        index += 2;
                        continue;
                    }
                }
                source_comments = SourceCommentMode::Readable;
                index += 1;
            }
            _ => return Command::Invalid,
        }
    }

    Command::Compile {
        path,
        map_path,
        source_comments,
    }
}

fn parse_init_command(args: &[OsString]) -> Command {
    let mut root = None;
    let mut name = None;
    let mut install_std = false;
    let mut output_root = None;
    let mut runtime_base = None;
    let mut autorun = true;
    let mut index = 0;

    while index < args.len() {
        match args[index].to_string_lossy().as_ref() {
            "--name" => {
                let Some(value) = args.get(index + 1).and_then(|arg| arg.to_str()) else {
                    return Command::Invalid;
                };
                name = Some(value.to_string());
                index += 2;
            }
            "--out" => {
                let Some(value) = args.get(index + 1) else {
                    return Command::Invalid;
                };
                output_root = Some(PathBuf::from(value));
                index += 2;
            }
            "--runtime-base" => {
                let Some(value) = args.get(index + 1) else {
                    return Command::Invalid;
                };
                runtime_base = Some(PathBuf::from(value));
                index += 2;
            }
            "--no-autorun" => {
                autorun = false;
                index += 1;
            }
            "--std" => {
                install_std = true;
                index += 1;
            }
            value if value.starts_with("--") => return Command::Invalid,
            _ => {
                if root.is_some() {
                    return Command::Invalid;
                }
                root = Some(PathBuf::from(&args[index]));
                index += 1;
            }
        }
    }

    let root = root.unwrap_or_else(|| PathBuf::from("."));
    let name = name.unwrap_or_else(|| {
        root.file_name()
            .map(|name| name.to_string_lossy().to_string())
            .filter(|name| !name.is_empty() && name != ".")
            .unwrap_or_else(|| "lux-project".into())
    });
    Command::Init(InitOptions {
        root,
        name,
        install_std,
        output_root,
        runtime_base,
        autorun,
    })
}

fn parse_install_command(args: &[OsString]) -> Command {
    let Some(package) = args
        .first()
        .and_then(|arg| arg.to_str())
        .map(str::to_string)
    else {
        return Command::Invalid;
    };
    let mut from = None;
    let mut tag = None;
    let mut branch = None;
    let mut commit = None;
    let mut project_root = PathBuf::from(".");
    let mut index = 1;

    while index < args.len() {
        match args[index].to_string_lossy().as_ref() {
            "--from" => {
                let Some(value) = args.get(index + 1).and_then(|arg| arg.to_str()) else {
                    return Command::Invalid;
                };
                from = Some(value.to_string());
                index += 2;
            }
            "--tag" => {
                let Some(value) = args.get(index + 1).and_then(|arg| arg.to_str()) else {
                    return Command::Invalid;
                };
                tag = Some(value.to_string());
                index += 2;
            }
            "--branch" => {
                let Some(value) = args.get(index + 1).and_then(|arg| arg.to_str()) else {
                    return Command::Invalid;
                };
                branch = Some(value.to_string());
                index += 2;
            }
            "--commit" => {
                let Some(value) = args.get(index + 1).and_then(|arg| arg.to_str()) else {
                    return Command::Invalid;
                };
                commit = Some(value.to_string());
                index += 2;
            }
            "--project" => {
                let Some(value) = args.get(index + 1) else {
                    return Command::Invalid;
                };
                project_root = PathBuf::from(value);
                index += 2;
            }
            _ => return Command::Invalid,
        }
    }

    if [tag.as_ref(), branch.as_ref(), commit.as_ref()]
        .into_iter()
        .flatten()
        .count()
        > 1
    {
        return Command::Invalid;
    }

    let Some(from) = from else {
        return Command::Invalid;
    };
    let Some(source) = parse_dependency_source(&from, tag, branch, commit) else {
        return Command::Invalid;
    };
    Command::Install(InstallRequest {
        project_root,
        package,
        source,
    })
}

fn parse_remove_command(args: &[OsString]) -> Command {
    let Some(package) = args
        .first()
        .and_then(|arg| arg.to_str())
        .map(str::to_string)
    else {
        return Command::Invalid;
    };
    let mut project_root = PathBuf::from(".");
    let mut index = 1;

    while index < args.len() {
        match args[index].to_string_lossy().as_ref() {
            "--project" => {
                let Some(value) = args.get(index + 1) else {
                    return Command::Invalid;
                };
                project_root = PathBuf::from(value);
                index += 2;
            }
            _ => return Command::Invalid,
        }
    }

    Command::Remove(RemoveRequest {
        project_root,
        package,
    })
}

fn parse_dependency_source(
    value: &str,
    tag: Option<String>,
    branch: Option<String>,
    commit: Option<String>,
) -> Option<DependencySource> {
    if let Some(repo) = value.strip_prefix("github:") {
        if repo.trim().is_empty() {
            return None;
        }
        return Some(DependencySource::Github {
            repo: repo.to_string(),
            tag,
            branch,
            commit,
        });
    }
    if value.starts_with("http://") || value.starts_with("https://") {
        return Some(DependencySource::Url(value.to_string()));
    }
    if tag.is_some() || branch.is_some() || commit.is_some() {
        return None;
    }
    Some(DependencySource::Path(PathBuf::from(value)))
}

fn parse_gmod_package_command(args: &[OsString]) -> Command {
    let mut manifest = None;
    let mut package_root = None;
    let mut output_root = None;
    let mut runtime_base = None;
    let mut autorun = None;
    let mut gmad_path = None;
    let mut output_gma = None;
    let mut run = false;
    let mut index = 0;

    while index < args.len() {
        match args[index].to_string_lossy().as_ref() {
            "--manifest" => {
                let Some(path) = args.get(index + 1) else {
                    return Command::Invalid;
                };
                manifest = Some(PathBuf::from(path));
                index += 2;
            }
            "--root" => {
                let Some(path) = args.get(index + 1) else {
                    return Command::Invalid;
                };
                package_root = Some(PathBuf::from(path));
                index += 2;
            }
            "--build-out" => {
                let Some(path) = args.get(index + 1) else {
                    return Command::Invalid;
                };
                output_root = Some(PathBuf::from(path));
                index += 2;
            }
            "--runtime-base" => {
                let Some(path) = args.get(index + 1) else {
                    return Command::Invalid;
                };
                runtime_base = Some(PathBuf::from(path));
                index += 2;
            }
            "--no-autorun" => {
                autorun = Some(false);
                index += 1;
            }
            "--gmad" => {
                let Some(path) = args.get(index + 1) else {
                    return Command::Invalid;
                };
                gmad_path = Some(PathBuf::from(path));
                index += 2;
            }
            "--out" => {
                let Some(path) = args.get(index + 1) else {
                    return Command::Invalid;
                };
                output_gma = Some(PathBuf::from(path));
                index += 2;
            }
            "--run" => {
                run = true;
                index += 1;
            }
            _ => return Command::Invalid,
        }
    }

    let (Some(manifest), Some(package_root), Some(gmad_path), Some(output_gma)) =
        (manifest, package_root, gmad_path, output_gma)
    else {
        return Command::Invalid;
    };

    Command::GmodPackage {
        manifest,
        package_root,
        output_root,
        runtime_base,
        autorun,
        gmad_path,
        output_gma,
        run,
    }
}

fn parse_gmod_build_command(args: &[OsString]) -> Command {
    let mut positionals = Vec::<PathBuf>::new();
    let mut manifest = None;
    let mut output_root = None;
    let mut runtime_base = None;
    let mut autorun = None;
    let mut dry_run = false;
    let mut index = 0;

    while index < args.len() {
        match args[index].to_string_lossy().as_ref() {
            "--manifest" => {
                let Some(path) = args.get(index + 1) else {
                    return Command::Invalid;
                };
                manifest = Some(PathBuf::from(path));
                index += 2;
            }
            "--out" => {
                let Some(path) = args.get(index + 1) else {
                    return Command::Invalid;
                };
                output_root = Some(PathBuf::from(path));
                index += 2;
            }
            "--runtime-base" => {
                let Some(path) = args.get(index + 1) else {
                    return Command::Invalid;
                };
                runtime_base = Some(PathBuf::from(path));
                index += 2;
            }
            "--no-autorun" => {
                autorun = Some(false);
                index += 1;
            }
            "--dry-run" => {
                dry_run = true;
                index += 1;
            }
            _ => {
                positionals.push(PathBuf::from(&args[index]));
                index += 1;
            }
        }
    }

    if manifest.is_some() && !positionals.is_empty() {
        return Command::Invalid;
    }

    let source_root = match positionals.as_slice() {
        [] => None,
        [source_root] => Some(source_root.clone()),
        _ => return Command::Invalid,
    };

    if manifest.is_none() && (source_root.is_none() || output_root.is_none()) {
        return Command::Invalid;
    }

    Command::GmodBuild {
        manifest,
        source_root,
        output_root,
        runtime_base,
        autorun,
        dry_run,
    }
}

fn parse_gmod_api_update_command(args: &[OsString]) -> Command {
    let mut forwarded = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].to_string_lossy();
        match arg.as_ref() {
            "--out" | "--coverage-out" | "--cache-dir" | "--source-url" | "--base-url"
            | "--override" | "--limit" | "--concurrency" => {
                let Some(value) = args.get(index + 1) else {
                    return Command::Invalid;
                };
                forwarded.push(arg.to_string());
                forwarded.push(value.to_string_lossy().to_string());
                index += 2;
            }
            "--no-coverage-out" | "--no-cache" | "--allow-failures" | "--offline" => {
                forwarded.push(arg.to_string());
                index += 1;
            }
            _ => return Command::Invalid,
        }
    }
    Command::GmodApiUpdate { args: forwarded }
}

fn map_error(map_path: PathBuf, generated_line: usize) -> Result<ExitCode, String> {
    match map_generated_line(&map_path, generated_line)? {
        Some(location) => {
            let file = location
                .source_file
                .unwrap_or_else(|| "<unknown source>".to_string());
            let line = location
                .source_line
                .map(|line| line.to_string())
                .unwrap_or_else(|| "?".to_string());
            let column = location
                .source_column
                .map(|column| column.to_string())
                .unwrap_or_else(|| "?".to_string());
            println!("{file}:{line}:{column}");
        }
        None => {
            println!(
                "no source mapping for generated line {} in {}",
                generated_line,
                map_path.display()
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn gmod_build(
    manifest: Option<PathBuf>,
    source_root: Option<PathBuf>,
    output_root: Option<PathBuf>,
    runtime_base: Option<PathBuf>,
    autorun: Option<bool>,
    dry_run: bool,
) -> Result<ExitCode, String> {
    let mut options = if let Some(manifest_path) = manifest {
        let manifest = ProjectManifest::load(&manifest_path).map_err(|err| err.to_string())?;
        GmodBuildOptions::from_manifest(manifest)
    } else {
        let source_root = source_root.expect("parse_command validates source root");
        let output_root = output_root
            .clone()
            .expect("parse_command validates output root");
        GmodBuildOptions::new(source_root, output_root)
    };

    if let Some(output_root) = output_root {
        options.output_root = output_root;
    }
    if let Some(runtime_base) = runtime_base {
        options.runtime_base = Some(runtime_base);
    }
    if let Some(autorun) = autorun {
        options.autorun = autorun;
    }
    options.write_files = !dry_run;
    let output = build_gmod_project(&options).map_err(|err| err.to_string())?;

    for diagnostic in &output.diagnostics {
        eprintln!("{}", diagnostic.message);
    }

    println!(
        "GMod build planned {} module(s), {} artifact(s)",
        output.build_plan.modules.len(),
        output.artifacts.len()
    );
    println!(
        "shared loader: {}",
        output.build_plan.loader.shared_loader.path.display()
    );
    println!(
        "client loader: {}",
        output.build_plan.loader.client_loader.path.display()
    );
    println!(
        "server loader: {}",
        output.build_plan.loader.server_loader.path.display()
    );
    if let Some(autorun) = &output.build_plan.autorun {
        println!("autorun forwarder: {}", autorun.path.display());
    } else {
        println!("autorun forwarder: disabled");
    }

    if dry_run {
        println!("dry run: no files written");
    } else {
        println!("wrote generated Lua, source maps, loader files, and optional autorun forwarder");
    }

    Ok(ExitCode::SUCCESS)
}

fn gmod_package(
    manifest_path: PathBuf,
    package_root: PathBuf,
    output_root: Option<PathBuf>,
    runtime_base: Option<PathBuf>,
    autorun: Option<bool>,
    gmad_path: PathBuf,
    output_gma: PathBuf,
    run: bool,
) -> Result<ExitCode, String> {
    let manifest = ProjectManifest::load(&manifest_path).map_err(|err| err.to_string())?;
    let mut options = GmodBuildOptions::from_manifest(manifest);
    if let Some(output_root) = output_root {
        options.output_root = output_root;
    }
    if let Some(runtime_base) = runtime_base {
        options.runtime_base = Some(runtime_base);
    }
    if let Some(autorun) = autorun {
        options.autorun = autorun;
    }
    options.write_files = true;

    let output = build_gmod_project(&options).map_err(|err| err.to_string())?;
    for diagnostic in &output.diagnostics {
        eprintln!("{}", diagnostic.message);
    }
    let args = vec![
        OsString::from("create"),
        OsString::from("-folder"),
        package_root.as_os_str().to_os_string(),
        OsString::from("-out"),
        output_gma.as_os_str().to_os_string(),
    ];

    println!("wrote generated Lua before optional package step");
    println!("package command:");
    println!(
        "  {} {}",
        gmad_path.display(),
        args.iter()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
    );

    if !run {
        println!("package dry run: pass --run to execute gmad");
        return Ok(ExitCode::SUCCESS);
    }

    let status = ProcessCommand::new(&gmad_path)
        .args(&args)
        .status()
        .map_err(|err| format!("failed to run {}: {err}", gmad_path.display()))?;
    if !status.success() {
        return Err(format!("gmad exited with status {status}"));
    }

    println!("wrote {}", output_gma.display());
    println!(
        "packaged {} module(s), {} artifact(s)",
        output.build_plan.modules.len(),
        output.artifacts.len()
    );
    Ok(ExitCode::SUCCESS)
}

fn gmod_api_update(args: Vec<String>) -> Result<ExitCode, String> {
    let summary = gmod_api_update::run_with_args(args)?;
    println!(
        "updated GMod API database: {} entries, {} hooks, {} classes",
        summary.entries, summary.hooks, summary.classes
    );
    println!(
        "coverage: {} official page(s), {} API candidate page(s), {} structured, {} fallback, {} failed",
        summary.official_pages,
        summary.api_candidate_pages,
        summary.structured_pages,
        summary.fallback_pages,
        summary.failed_pages
    );
    println!("database: {}", summary.database_path.display());
    if let Some(path) = summary.coverage_path {
        println!("coverage manifest: {}", path.display());
    }
    Ok(ExitCode::SUCCESS)
}

fn package_init(options: InitOptions) -> Result<ExitCode, String> {
    init_project(&options).map_err(|err| err.to_string())?;
    println!("initialized Lux project at {}", options.root.display());
    if options.install_std {
        println!("installed @lux/std from github:TimeWatcher/lux-std");
    }
    Ok(ExitCode::SUCCESS)
}

fn package_install(request: InstallRequest) -> Result<ExitCode, String> {
    let output = install_package(&request).map_err(|err| err.to_string())?;
    println!(
        "installed {} into {} ({} direct, {} total packages)",
        output.package_id,
        output.package_root.display(),
        output.direct_count,
        output.total_count
    );
    println!("lockfile: {}", output.lock_path.display());
    Ok(ExitCode::SUCCESS)
}

fn package_remove(request: RemoveRequest) -> Result<ExitCode, String> {
    let output = remove_package(&request).map_err(|err| err.to_string())?;
    println!(
        "removed {} ({} direct, {} total packages)",
        output.package_id, output.direct_count, output.total_count
    );
    println!("lockfile: {}", output.lock_path.display());
    Ok(ExitCode::SUCCESS)
}

fn package_lock(project_root: PathBuf) -> Result<ExitCode, String> {
    let output = lock_project(&LockRequest { project_root }).map_err(|err| err.to_string())?;
    println!(
        "locked {} direct, {} total packages in {}",
        output.direct_count,
        output.total_count,
        output.project_root.display()
    );
    println!("lockfile: {}", output.lock_path.display());
    Ok(ExitCode::SUCCESS)
}

fn package_list(project_root: PathBuf) -> Result<ExitCode, String> {
    let packages = list_locked(&project_root).map_err(|err| err.to_string())?;
    if packages.is_empty() {
        println!("no locked packages in {}", project_root.display());
        return Ok(ExitCode::SUCCESS);
    }
    for package in packages {
        println!(
            "{} {} {} {}",
            package.id,
            package.version,
            if package.direct {
                "direct"
            } else {
                "transitive"
            },
            package.root.display()
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn package_doctor_command(project_root: PathBuf) -> Result<ExitCode, String> {
    let report = package_doctor(&project_root).map_err(|err| err.to_string())?;
    println!("project: {}", report.project_root.display());
    println!("manifest: {}", report.manifest_path.display());
    println!("lockfile: {}", report.lock_path.display());
    println!("direct dependencies: {}", report.dependency_count);
    println!("locked packages: {}", report.locked_count);
    for root in report.package_roots {
        println!("package root: {}", root.display());
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn init_defaults_to_no_std_dependency() {
        let Command::Init(options) = parse_command(args(&["init", "demo"])) else {
            panic!("expected init command");
        };

        assert_eq!(options.root, PathBuf::from("demo"));
        assert_eq!(options.name, "demo");
        assert!(!options.install_std);
        assert_eq!(options.output_root, None);
        assert_eq!(options.runtime_base, None);
        assert!(options.autorun);
    }

    #[test]
    fn init_std_requests_official_std_install() {
        let Command::Init(options) = parse_command(args(&["init", "demo", "--std"])) else {
            panic!("expected init command");
        };

        assert!(options.install_std);
    }

    #[test]
    fn init_accepts_gmod_output_controls() {
        let Command::Init(options) = parse_command(args(&[
            "init",
            "demo",
            "--out",
            "generated",
            "--runtime-base",
            "framework/lux/demo",
            "--no-autorun",
        ])) else {
            panic!("expected init command");
        };

        assert_eq!(options.output_root, Some(PathBuf::from("generated")));
        assert_eq!(
            options.runtime_base,
            Some(PathBuf::from("framework/lux/demo"))
        );
        assert!(!options.autorun);
    }

    #[test]
    fn gmod_build_accepts_output_controls() {
        let Command::GmodBuild {
            manifest,
            source_root,
            output_root,
            runtime_base,
            autorun,
            dry_run,
        } = parse_command(args(&[
            "gmod",
            "build",
            "src",
            "--out",
            "generated",
            "--runtime-base",
            "lux/demo",
            "--no-autorun",
            "--dry-run",
        ]))
        else {
            panic!("expected gmod build command");
        };

        assert_eq!(manifest, None);
        assert_eq!(source_root, Some(PathBuf::from("src")));
        assert_eq!(output_root, Some(PathBuf::from("generated")));
        assert_eq!(runtime_base, Some(PathBuf::from("lux/demo")));
        assert_eq!(autorun, Some(false));
        assert!(dry_run);
    }

    #[test]
    fn gmod_build_rejects_second_positional_path() {
        assert!(matches!(
            parse_command(args(&["gmod", "build", "src", "addon"])),
            Command::Invalid
        ));
    }

    #[test]
    fn install_rejects_removed_builtin_source() {
        assert!(matches!(
            parse_command(args(&["install", "@lux/std", "--builtin"])),
            Command::Invalid
        ));
    }

    #[test]
    fn remove_accepts_project_root() {
        let Command::Remove(request) =
            parse_command(args(&["remove", "@lux/gmod", "--project", "demo"]))
        else {
            panic!("expected remove command");
        };

        assert_eq!(request.package, "@lux/gmod");
        assert_eq!(request.project_root, PathBuf::from("demo"));
    }

    #[test]
    fn remove_rejects_unknown_flags() {
        assert!(matches!(
            parse_command(args(&[
                "remove",
                "@lux/gmod",
                "--from",
                "github:vendor/pkg"
            ])),
            Command::Invalid
        ));
    }

    #[test]
    fn lock_defaults_to_current_project() {
        let Command::Lock { project_root } = parse_command(args(&["lock"])) else {
            panic!("expected lock command");
        };

        assert_eq!(project_root, PathBuf::from("."));
    }

    #[test]
    fn lock_accepts_project_root() {
        let Command::Lock { project_root } = parse_command(args(&["lock", "demo"])) else {
            panic!("expected lock command");
        };

        assert_eq!(project_root, PathBuf::from("demo"));
    }
}
