use anyhow::{anyhow, Context};
use cargo_metadata::camino::Utf8PathBuf;
use cargo_metadata::Message;
use carol_core::{BinaryId, MachineId};
use carol_host::{CompiledBinary, Executor};
use clap::{Args, Parser, Subcommand};
use clap_cargo::Workspace;
use std::{
    process::{Command, Stdio},
    str::FromStr,
};
use wit_component::ComponentEncoder;

mod client;
use client::Client;

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let subscriber = tracing_subscriber::fmt()
        .with_max_level(cli.log_level)
        .pretty()
        .finish();

    tracing::subscriber::set_global_default(subscriber)?;

    match cli.command {
        Commands::Build(opts) => println!("{}", opts.run(&Executor::new())?.0),
        Commands::Upload(opts) => {
            let server_opt = &opts.server;
            let binary_id = opts.run(&Executor::new(), &server_opt.new_client())?;
            if cli.quiet {
                println!("{}", binary_id)
            } else {
                println!(
                    "{}",
                    server_opt.url_for(&format!("/binaries/{}", binary_id))
                );
            }
        }
        Commands::Create(opts) => {
            let server_opt = &opts.implied_upload.server;
            let (_, machine_id) = opts.run(&Executor::new(), &server_opt.new_client())?;
            if cli.quiet {
                println!("{}", machine_id);
            } else {
                println!(
                    "url: {}",
                    server_opt.url_for(&format!("/machines/{}", machine_id))
                );
                println!(
                    "http-root: {}",
                    server_opt.url_for(&format!("/machines/{}/http", machine_id))
                );
                if let Some(cname) = server_opt.cname_for_machine(machine_id) {
                    println!("cname domain: {}", cname);
                }
            }
        }
        Commands::Api(opts) => {
            let activations = opts.run(&Executor::new())?;
            println!("{}", activations.join("\n"));
        }
        Commands::Run(opts) => {
            opts.run(&Executor::new())?;
        }
    };

    Ok(())
}

/// carlo: command line interface for Carol
#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Write minial representation of output to stdout
    /// e.g. instead of outputing the full url to the resource just output the id.
    #[clap(short, long)]
    quiet: bool,
    #[clap(long, default_value = "info")]
    log_level: tracing::Level,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Build(BuildOpts),
    Upload(UploadOpts),
    Create(CreateOpts),
    Api(ApiOpts),
    Run(RunOpts),
}

/// Inspect
#[derive(Args, Debug)]
pub struct ApiOpts {
    #[clap(flatten)]
    implied_build: BuildOpts,
    #[arg(
        long,
        value_name = "WASM_FILE",
        group = "api",
        conflicts_with = "build"
    )]
    binary: Option<Utf8PathBuf>,
}

#[derive(Args, Debug)]
/// Compile a Carol WASM component binary from a Rust crate
struct BuildOpts {
    #[arg(short, long, value_name = "SPEC", group = "build")]
    /// Package to compile to a Carol WASM component (see `cargo help pkgid`)
    pub package: Option<String>, // real one has Vec<String>
}

#[derive(Args, Debug)]
/// Upload a component binary to a Carol server
struct UploadOpts {
    /// The binary (WASM component) to upload (implied by --package)
    #[arg(
        long,
        value_name = "WASM_FILE",
        group = "upload",
        conflicts_with = "build"
    )]
    binary: Option<Utf8PathBuf>,

    #[clap(flatten)]
    implied_build: BuildOpts,

    #[clap(flatten)]
    server: ServerOpts,
}

#[derive(Args, Debug)]
/// Create a machine from a component binary on a Carol server
struct CreateOpts {
    /// The ID of the compiled binary from which to create a machine (implied by --binary)
    #[arg(
        long,
        value_name = "BINARY-ID",
        group = "create",
        conflicts_with = "upload",
        conflicts_with = "build"
    )]
    binary_id: Option<BinaryId>,

    #[clap(flatten)]
    implied_upload: UploadOpts,
}

#[derive(Args, Debug, Clone)]
struct ServerOpts {
    #[arg(long)] // , default_value = "http://localhost:8000")] ?
    carol_url: reqwest::Url,
}

impl ServerOpts {
    pub fn new_client(&self) -> Client {
        Client::new(self.carol_url.clone())
    }

    pub fn cname_for_machine(&self, id: MachineId) -> Option<String> {
        let host = self.carol_url.host()?;
        if let url::Host::Domain(domain) = host {
            Some(format!(
                "{}.{}",
                carol_http::host_header_label_for_machine(id),
                domain
            ))
        } else {
            None
        }
    }

    pub fn url_for(&self, path: &str) -> reqwest::Url {
        self.carol_url.join(path).expect("path is valid")
    }
}

impl BuildOpts {
    fn run(&self, exec: &Executor) -> anyhow::Result<(Utf8PathBuf, CompiledBinary)> {
        // Find the crate package to compile
        let metadata = cargo_metadata::MetadataCommand::new()
            .exec()
            .context("Couldn't build Carol WASM component")?;
        let mut ws = Workspace::default();
        ws.package = self.package.iter().cloned().collect();
        let (included, _) = ws.partition_packages(&metadata);
        if included.is_empty() {
            return Err(anyhow!(
                "package ID specification {:?} did not match any packages",
                ws.package
            ));
        }
        if included.len() != 1 {
            return Err(anyhow!("Carol WASM components must be built from a single crate, but package ID specification {:?} resulted in {} packages (did you forget to specify -p in a workspace?)", ws.package, included.len()));
        }
        let package = &included[0].name;

        // Compile to WASM target
        // TODO use cargo::ops::compile instead of invoking cargo CLI?
        let mut cmd = Command::new("cargo");
        cmd.env("RUSTFLAGS", "-C opt-level=z")
            .args([
                "rustc",
                "--package",
                package,
                "--message-format=json-render-diagnostics",
                "--target",
                "wasm32-unknown-unknown",
                "--release",
                "--crate-type=cdylib",
            ])
            .stdout(Stdio::piped());

        eprintln!("Running {:?}", cmd);

        let mut proc = cmd.spawn().context("Couldn't spawn cargo rustc")?;

        let reader = std::io::BufReader::new(proc.stdout.take().unwrap());
        let messages = cargo_metadata::Message::parse_stream(reader)
            .collect::<Result<Vec<_>, _>>()
            .context("Couldn't read cargo output")?;

        let output = proc
            .wait_with_output()
            .context("Couldn't read `cargo rustc` output")?;

        if !output.status.success() {
            return Err(anyhow!(
                "`cargo rustc` exited unsuccessfully ({})",
                output.status
            ));
        }

        // Find the last compiler artifact message
        let final_artifact_message = messages
            .into_iter()
            .rev()
            .find_map(|message| match message {
                Message::CompilerArtifact(artifact) => Some(artifact),
                _ => None,
            })
            .ok_or_else(|| {
                anyhow!("No compiler artifact messages in output, could not find wasm output file.")
            })?;

        if final_artifact_message.filenames.len() != 1 {
            return Err(anyhow!(
                "Expected a single wasm artifact in files, but got:\n{}",
                final_artifact_message
                    .filenames
                    .iter()
                    .enumerate()
                    .map(|(i, name)| format!("{}: {}", i, name))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }

        let final_wasm_artifact = final_artifact_message.filenames[0].clone();

        let component_target = append_to_basename(&final_wasm_artifact, "-component")?;

        // Encode the component and write artifcat
        let wasm = std::fs::read(&final_wasm_artifact).context(format!(
            "Couldn't read compiled WASM file {final_wasm_artifact}"
        ))?;

        let encoder = ComponentEncoder::default()
            .validate(true)
            .module(&wasm)
            .context(format!(
                "validating wasm while transforming {final_wasm_artifact} into a component"
            ))?;

        let bytes = encoder
            .encode()
            .context("Failed to encode a component from module")?;

        std::fs::write(&component_target, bytes)
            .context(format!("Couldn't write WASM component {component_target}"))?;

        // TODO remove or (after careful consideration) convert to a
        // warning before release, as this strongly assumes the client side
        // carlo binary and server side carol host exactly agree on the
        // definition of Executor::load_binary_from_wasm_file.
        let compiled = exec
            .load_binary_from_wasm_file(&component_target)
            .context(format!(
                "Compiled WASM component {component_target} was invalid"
            ))?;

        Ok((component_target, compiled))
    }
}

impl UploadOpts {
    fn run(&self, exec: &Executor, client: &Client) -> anyhow::Result<BinaryId> {
        let binary = match &self.binary {
            Some(binary) => binary.clone(),
            None => {
                self.implied_build
                    .run(exec)
                    .context("Failed to build crate for upload")?
                    .0
            }
        };

        // Validate and derive BinaryId
        let binary_id = exec
            .load_binary_from_wasm_file(&binary)
            .context("Couldn't load compiled binary")?
            .binary_id();

        let file =
            std::fs::File::open(&binary).context(format!("Couldn't read file {}", binary))?;

        let response = client.upload_binary(&binary_id, file)?;
        let binary_id = response.id;
        Ok(binary_id)
    }
}

impl CreateOpts {
    fn run(&self, exec: &Executor, client: &Client) -> anyhow::Result<(BinaryId, MachineId)> {
        let binary_id = match self.binary_id {
            Some(binary_id) => binary_id,
            None => self
                .implied_upload
                .run(exec, client)
                .context("Failed to upload binary for machine creation")?,
        };

        let response = client.create_machine(&binary_id)?;
        let machine_id = response.id;
        Ok((binary_id, machine_id))
    }
}

impl ApiOpts {
    fn run(&self, exec: &Executor) -> anyhow::Result<Vec<String>> {
        let compiled = match &self.binary {
            Some(binary) => exec.load_binary_from_wasm_file(binary)?,
            None => {
                self.implied_build
                    .run(exec)
                    .context("Failed to build crate for upload")?
                    .1
            }
        };

        let binary_api =
            tokio::runtime::Runtime::new()?.block_on(exec.get_binary_api(&compiled))?;
        Ok(binary_api
            .activations
            .into_iter()
            .map(|activation| activation.name)
            .collect())
    }
}

#[derive(Args, Debug)]
/// Build and then run the machine on a carol server for testing purposes.
///
/// The server will have an insecure (dummy) keypair.
pub struct RunOpts {
    /// The binary (WASM component) to upload (implied by --package)
    #[arg(
        long,
        value_name = "WASM_FILE",
        group = "upload",
        conflicts_with = "build"
    )]
    binary: Option<Utf8PathBuf>,

    #[clap(flatten)]
    implied_build: BuildOpts,

    /// Where the temporary server should listen
    #[clap(short, long, default_value = "127.0.0.0:0")]
    listen: std::net::SocketAddr,
}

impl RunOpts {
    fn run(self, exec: &Executor) -> anyhow::Result<()> {
        let state = carol_host::State::new(carol_bls::KeyPair::from_bytes([42u8; 32]).unwrap());
        let rt = tokio::runtime::Runtime::new()?;
        let _enter_guard = rt.enter();
        let http_server_config = carol::config::HttpServerConfig {
            listen: self.listen,
            ..Default::default()
        };

        let (bound_addr, server) = carol::http::server::start(http_server_config, state)
            .expect("should be able to start HTTP server");
        let handle = rt.spawn(server);
        let server_opts = ServerOpts {
            carol_url: reqwest::Url::from_str(&format!("http://{bound_addr}"))
                .expect("this should be valid"),
        };
        let client = server_opts.new_client();

        let implied_create = CreateOpts {
            binary_id: None,
            implied_upload: UploadOpts {
                binary: self.binary,
                implied_build: self.implied_build,
                server: server_opts.clone(),
            },
        };
        let (binary_id, machine_id) = implied_create.run(exec, &client)?;

        eprintln!("=== 🤖 MACHINE CREATED 🤖 ===");
        println!("binary_id={binary_id}");
        println!("machine_id={machine_id}");
        println!("carol_url={}", server_opts.url_for(""));
        println!(
            "binary_url={}",
            server_opts.url_for(&format!("/binaries/{binary_id}"))
        );
        println!(
            "machine_url={}",
            server_opts.url_for(&format!("/machines/{machine_id}"))
        );
        println!(
            "machine_http_url={}",
            server_opts.url_for(&format!("/machines/{machine_id}/http/"))
        );

        rt.block_on(handle)?;

        Ok(())
    }
}

/// Helper function for rewriting filenames while retaining extension
fn append_to_basename(path: &Utf8PathBuf, suffix: &str) -> anyhow::Result<Utf8PathBuf> {
    let ext = path
        .extension()
        .context("Expected path to contain an extension")?
        .to_string();

    let basename = path
        .file_stem()
        .context("Expected path to contain a file basename component")?;

    let mut path = path.clone();
    path.set_file_name(format!("{basename}{suffix}"));
    path.set_extension(ext);
    Ok(path)
}
