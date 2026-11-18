use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::HashMap, io::{BufRead, BufReader, Write}, path::PathBuf,
    process::{Child, Command, Stdio}, sync::mpsc, thread, time::{Duration, Instant, SystemTime, UNIX_EPOCH}};

pub fn now() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() }

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Quota {
    pub used_percent: f64,
    pub window_duration_mins: Option<u32>,
    pub resets_at: Option<u64>,
}
impl Quota {
    pub fn expired(&self) -> bool { self.resets_at.is_some_and(|reset| reset <= now()) }
    pub fn fraction(&self, remaining: bool) -> f32 {
        if self.expired() { return 0.; }
        (if remaining { 100. - self.used_percent } else { self.used_percent }) as f32 / 100.
    }
    pub fn label(&self, remaining: bool) -> String {
        if self.expired() { "--".into() } else { format!("{:.0}%", self.fraction(remaining) * 100.) }
    }
    pub fn reset_text(&self) -> String {
        let Some(reset) = self.resets_at else { return "Reset time unavailable".into(); };
        if reset <= now() { return "Waiting for reset confirmation".into(); }
        let mins = (reset - now()).div_ceil(60);
        if mins >= 1440 { format!("Resets in {} d {} h", mins / 1440, (mins % 1440) / 60) }
        else { format!("Resets in {} h {} min", mins / 60, mins % 60) }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Snapshot { pub five: Option<Quota>, pub week: Option<Quota>, pub fetched_at: u64 }

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Bucket { limit_id: Option<String>, primary: Option<Quota>, secondary: Option<Quota> }
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Response { rate_limits: Option<Bucket>, rate_limits_by_limit_id: Option<HashMap<String, Bucket>> }

pub fn parse(result: Value) -> Result<Snapshot, String> {
    let response: Response = serde_json::from_value(result).map_err(|_| "Respuesta de cuota no válida")?;
    let bucket = match response.rate_limits_by_limit_id {
        Some(mut buckets) => buckets.remove("codex"), None => response.rate_limits,
    }.ok_or("No está disponible la cuota general de Codex")?;
    if bucket.limit_id.as_ref().is_some_and(|id| id != "codex") {
        return Err("No está disponible la cuota general de Codex".into());
    }
    let mut snapshot = Snapshot { fetched_at: now(), ..Default::default() };
    for quota in [bucket.primary, bucket.secondary].into_iter().flatten() {
        if !quota.used_percent.is_finite() || !(0. ..=100.).contains(&quota.used_percent) { continue; }
        match quota.window_duration_mins {
            Some(300) => snapshot.five = Some(quota),
            Some(10080) => snapshot.week = Some(quota), _ => (),
        }
    }
    if snapshot.five.is_none() && snapshot.week.is_none() { return Err("No hay cuota de 5 horas o semanal disponible".into()); }
    Ok(snapshot)
}

fn executable() -> Result<PathBuf, String> {
    #[cfg(windows)]
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        for suffix in ["Programs/OpenAI/Codex/bin/codex.exe", "OpenAI/Codex/bin/codex.exe"] {
            let path = PathBuf::from(&local).join(suffix); if path.is_file() { return Ok(path); }
        }
    }
    for directory in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        let path = directory.join(if cfg!(windows) { "codex.exe" } else { "codex" });
        if path.is_file() { return Ok(path); }
    }
    Err("No se encontró Codex. Instálalo e inicia sesión.".into())
}

struct ChildGuard(Child);
impl Drop for ChildGuard { fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); } }

pub fn read() -> Result<Snapshot, String> {
    let mut command = Command::new(executable()?);
    command.args(["--no-daemon", "app-server", "--stdio"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    if let Some(home) = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }) { command.current_dir(home); }
    #[cfg(windows)] {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let mut child = ChildGuard(command.spawn().map_err(|_| "No se pudo iniciar el lector de Codex")?);
    let output = child.0.stdout.take().ok_or("No hay conexión con Codex")?;
    let mut input = child.0.stdin.take().ok_or("No hay conexión con Codex")?;
    let (sender, receiver) = mpsc::channel();
    let stream = thread::spawn(move || {
        for line in BufReader::new(output).lines() {
            let Ok(line) = line else { break; };
            if sender.send(line).is_err() { break; }
        }
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    let result = (|| {
        writeln!(input, "{}", json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"codex_quota_panel","title":"Codex Quota","version":"2.0.0"}}})).map_err(|_| "Codex cerró la conexión")?;
        input.flush().map_err(|_| "Codex cerró la conexión")?;
        receive(&receiver, 1, deadline)?;
        writeln!(input, "{}", json!({"method":"initialized"})).map_err(|_| "Codex cerró la conexión")?;
        writeln!(input, "{}", json!({"id":2,"method":"account/rateLimits/read"})).map_err(|_| "Codex cerró la conexión")?;
        input.flush().map_err(|_| "Codex cerró la conexión")?;
        parse(receive(&receiver, 2, deadline)?)
    })();
    drop(input); drop(child); drop(receiver); let _ = stream.join();
    result
}

fn receive(receiver: &mpsc::Receiver<String>, id: u32, deadline: Instant) -> Result<Value, String> {
    loop {
        let timeout = deadline.checked_duration_since(Instant::now()).ok_or("Codex tardó demasiado en responder")?;
        let line = receiver.recv_timeout(timeout).map_err(|_| "Sin respuesta de Codex. Revisa la sesión y conexión.")?;
        let Ok(value) = serde_json::from_str::<Value>(&line) else { continue; };
        if value["id"] != id { continue; }
        if value.get("error").is_some() { return Err("No se pudo consultar la cuota. Revisa tu sesión y conexión.".into()); }
        return value.get("result").cloned().ok_or("Respuesta de Codex no válida".into());
    }
}

pub struct Worker { pub refresh: mpsc::Sender<()>, pub results: mpsc::Receiver<Result<Snapshot, String>> }
impl Worker {
    pub fn start() -> Self {
        let (refresh, request) = mpsc::channel(); let (result, results) = mpsc::channel();
        thread::spawn(move || loop {
            if result.send(read()).is_err() { break; }
            // Collapse bursts of manual refresh requests into at most one following read.
            match request.recv_timeout(Duration::from_secs(120)) {
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                _ => { while request.try_recv().is_ok() {} }
            }
        });
        Self { refresh, results }
    }
}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn selects_general_bucket_and_actual_durations() {
        let result = parse(json!({"rateLimitsByLimitId":{"codex":{"primary":{"usedPercent":32,"windowDurationMins":10080},"secondary":{"usedPercent":17,"windowDurationMins":300}},"other":{"primary":{"usedPercent":99,"windowDurationMins":300}}}})).unwrap();
        assert_eq!(result.five.unwrap().label(true), "83%"); assert_eq!(result.week.unwrap().label(false), "32%");
    }
    #[test] fn supports_missing_window_and_zero() {
        let s = parse(json!({"rateLimits":{"primary":{"usedPercent":0,"windowDurationMins":300}}})).unwrap();
        assert_eq!(s.five.unwrap().label(true), "100%"); assert!(s.week.is_none());
    }
    #[test] fn rejects_unrelated_unknown_or_invalid_windows() {
        for value in [json!({}), json!({"rateLimitsByLimitId":{"other":{}},"rateLimits":{"primary":{"usedPercent":10,"windowDurationMins":300}}}), json!({"rateLimits":{"primary":{"usedPercent":101,"windowDurationMins":300}}}), json!({"rateLimits":{"limitId":"other","primary":{"usedPercent":1,"windowDurationMins":300}}}), json!({"rateLimits":{"primary":{"usedPercent":10,"windowDurationMins":15}}})] { assert!(parse(value).is_err()); }
    }
    #[test] fn expired_reset_never_invents_recovery() {
        let q = Quota { used_percent: 100., resets_at: Some(1), window_duration_mins: Some(300) };
        assert_eq!(q.label(true), "--"); assert_eq!(q.fraction(true), 0.);
    }
    #[test] fn rpc_rejects_error_without_exposing_server_details() {
        let (tx, rx) = mpsc::channel(); tx.send(r#"{"id":2,"error":{"message":"SECRET"}}"#.into()).unwrap();
        let error = receive(&rx, 2, Instant::now() + Duration::from_secs(1)).unwrap_err();
        assert!(!error.contains("SECRET"));
    }
}
