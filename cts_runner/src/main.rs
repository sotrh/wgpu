use std::{
    env, fmt,
    io::{Read, Write},
    rc::Rc,
};

use deno_core::anyhow::anyhow;
use deno_core::error::AnyError;
use deno_core::located_script_name;
use deno_core::resolve_url_or_path;
use deno_core::serde_json;
use deno_core::serde_json::json;
use deno_core::JsRuntime;
use deno_core::OpState;
use deno_core::RuntimeOptions;
use deno_core::ZeroCopyBuf;
use deno_web::BlobStore;
use termcolor::Ansi;
use termcolor::Color::Red;
use termcolor::ColorSpec;
use termcolor::WriteColor;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    unwrap_or_exit(run().await)
}

async fn run() -> Result<(), AnyError> {
    let mut args_iter = env::args();
    let _ = args_iter.next();
    let url = args_iter
        .next()
        .ok_or_else(|| anyhow!("missing specifier in first command line argument"))?;
    let specifier = resolve_url_or_path(&url)?;

    let options = RuntimeOptions {
        module_loader: Some(Rc::new(deno_core::FsModuleLoader)),
        get_error_class_fn: Some(&get_error_class_name),
        extensions: vec![
            deno_webidl::init(),
            deno_console::init(),
            deno_url::init(),
            deno_web::init(BlobStore::default(), None),
            deno_timers::init::<Permissions>(),
            deno_webgpu::init(true),
            extension(),
        ],
        ..Default::default()
    };
    let mut isolate = JsRuntime::new(options);
    let args = args_iter.collect::<Vec<String>>();
    let cfg = json!({"args": args, "cwd": env::current_dir().unwrap().to_string_lossy() });
    let bootstrap_script = format!("globalThis.bootstrap({})", serde_json::to_string(&cfg)?);
    isolate.execute_script(&located_script_name!(), &bootstrap_script)?;

    isolate
        .op_state()
        .borrow_mut()
        .put(Permissions{});

    let mod_id = isolate.load_main_module(&specifier, None).await?;
    let mod_rx = isolate.mod_evaluate(mod_id);

    let rx = tokio::spawn(async move {
        match mod_rx.await {
            Ok(err @ Err(_)) => err,
            _ => Ok(()),
        }
    });

    isolate.run_event_loop(false).await?;
    rx.await.unwrap()?;

    Ok(())
}

fn extension() -> deno_core::Extension {
    deno_core::Extension::builder()
        .ops(vec![
            ("op_exit", deno_core::op_sync(op_exit)),
            ("op_read_file_sync", deno_core::op_sync(op_read_file_sync)),
            ("op_write_file_sync", deno_core::op_sync(op_write_file_sync)),
        ])
        .js(deno_core::include_js_files!(
          prefix "deno:cts_runner",
          "src/bootstrap.js",
        ))
        .build()
}

fn op_exit(_state: &mut OpState, code: i32, _: ()) -> Result<(), AnyError> {
    std::process::exit(code)
}

fn op_read_file_sync(_state: &mut OpState, path: String, _: ()) -> Result<ZeroCopyBuf, AnyError> {
    let path = std::path::Path::new(&path);
    let mut file = std::fs::File::open(path)?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    Ok(ZeroCopyBuf::from(buf))
}

fn op_write_file_sync(
    _state: &mut OpState,
    path: String,
    buf: ZeroCopyBuf,
) -> Result<(), AnyError> {
    let path = std::path::Path::new(&path);
    let mut file = std::fs::File::create(path)?;
    file.write_all(&buf)?;
    Ok(())
}

fn get_error_class_name(e: &AnyError) -> &'static str {
    deno_core::error::get_custom_error_class(e)
        .or_else(|| deno_webgpu::error::get_error_class_name(e))
        .unwrap_or_else(|| {
            panic!(
                "Error '{}' contains boxed error of unsupported type:{}",
                e,
                e.chain()
                    .map(|e| format!("\n  {:?}", e))
                    .collect::<String>()
            );
        })
}

fn unwrap_or_exit<T>(result: Result<T, AnyError>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => {
            eprintln!("{}: {:?}", red_bold("error"), error);
            std::process::exit(1);
        }
    }
}

fn style<S: AsRef<str>>(s: S, colorspec: ColorSpec) -> impl fmt::Display {
    let mut v = Vec::new();
    let mut ansi_writer = Ansi::new(&mut v);
    ansi_writer.set_color(&colorspec).unwrap();
    ansi_writer.write_all(s.as_ref().as_bytes()).unwrap();
    ansi_writer.reset().unwrap();
    String::from_utf8_lossy(&v).into_owned()
}

fn red_bold<S: AsRef<str>>(s: S) -> impl fmt::Display {
    let mut style_spec = ColorSpec::new();
    style_spec.set_fg(Some(Red)).set_bold(true);
    style(s, style_spec)
}

// NOP permissions
struct Permissions;

impl deno_timers::TimersPermission for Permissions {
    fn allow_hrtime(&mut self) -> bool {
        false
    }

    fn check_unstable(&self, _state: &deno_core::OpState, _api_name: &'static str) {}
}
