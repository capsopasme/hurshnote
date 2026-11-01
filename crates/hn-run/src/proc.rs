//! 子进程：启动、读取输出、等待退出、停止。
//!
//! stdout 与 stderr 各由一个线程读取（只用一个线程会在其中一个管道写满时死锁），
//! 另一个线程等待进程退出。输出通过回调交给调用方，回调在这些后台线程中执行。
//!
//! Windows：进程以 `CREATE_NO_WINDOW` 启动（不弹控制台窗口），并放进一个带
//! `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` 的 Job Object：`stop` 结束整棵进程树；
//! `Running` 被丢弃（例如程序退出）时，Job 句柄关闭，残留的子进程也会被结束。

use std::io::{self, Read};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;

/// 要运行的命令。
#[derive(Debug, Clone, Default)]
pub struct Spec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
}

/// 后台线程发出的事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
    /// 进程退出（退出码；被强制结束时可能为 None）
    Exit(Option<i32>),
}

pub type Sink = Arc<dyn Fn(Event) + Send + Sync>;

#[cfg(windows)]
mod job {
    use std::ffi::c_void;

    #[repr(C)]
    #[allow(non_snake_case)]
    struct BasicLimit {
        PerProcessUserTimeLimit: i64,
        PerJobUserTimeLimit: i64,
        LimitFlags: u32,
        MinimumWorkingSetSize: usize,
        MaximumWorkingSetSize: usize,
        ActiveProcessLimit: u32,
        Affinity: usize,
        PriorityClass: u32,
        SchedulingClass: u32,
    }

    #[repr(C)]
    #[allow(non_snake_case)]
    struct ExtendedLimit {
        Basic: BasicLimit,
        IoInfo: [u64; 6],
        ProcessMemoryLimit: usize,
        JobMemoryLimit: usize,
        PeakProcessMemoryUsed: usize,
        PeakJobMemoryUsed: usize,
    }

    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: i32 = 9;
    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;

    extern "system" {
        fn CreateJobObjectW(attrs: *const c_void, name: *const u16) -> *mut c_void;
        fn SetInformationJobObject(
            job: *mut c_void,
            class: i32,
            info: *const c_void,
            len: u32,
        ) -> i32;
        fn AssignProcessToJobObject(job: *mut c_void, process: *mut c_void) -> i32;
        fn TerminateJobObject(job: *mut c_void, code: u32) -> i32;
        fn CloseHandle(h: *mut c_void) -> i32;
    }

    /// Job Object 句柄（关闭时结束其中的所有进程）。
    pub struct Job(*mut c_void);

    // 安全：Job 句柄可以在线程间传递和共享，相关 API 都是线程安全的
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        pub fn new() -> Option<Job> {
            unsafe {
                let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if h.is_null() {
                    return None;
                }
                let mut info: ExtendedLimit = std::mem::zeroed();
                info.Basic.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                SetInformationJobObject(
                    h,
                    JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                    &info as *const _ as *const c_void,
                    std::mem::size_of::<ExtendedLimit>() as u32,
                );
                Some(Job(h))
            }
        }

        pub fn assign(&self, process: *mut c_void) -> bool {
            unsafe { AssignProcessToJobObject(self.0, process) != 0 }
        }

        pub fn terminate(&self) {
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

/// 运行中的进程。
pub struct Running {
    pid: u32,
    #[cfg(windows)]
    job: Option<job::Job>,
}

impl Running {
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// 结束进程（Windows 上结束整棵进程树）。退出事件仍会照常发出。
    pub fn stop(&self) {
        #[cfg(windows)]
        if let Some(j) = &self.job {
            j.terminate();
        }
        #[cfg(unix)]
        {
            let _ = Command::new("kill")
                .arg("-9")
                .arg(self.pid.to_string())
                .status();
        }
    }
}

fn pump(mut r: impl Read + Send + 'static, sink: Sink, err: bool) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            match r.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let data = buf[..n].to_vec();
                    sink(if err {
                        Event::Stderr(data)
                    } else {
                        Event::Stdout(data)
                    });
                }
            }
        }
    })
}

/// 启动进程。stdin 为空（`input()` 会立即得到 EOF）。
pub fn spawn(spec: &Spec, sink: Sink) -> io::Result<Running> {
    let mut cmd = Command::new(&spec.program);
    cmd.args(&spec.args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(d) = &spec.cwd {
        cmd.current_dir(d);
    }
    for (k, v) in &spec.env {
        cmd.env(k, v);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn()?;
    #[cfg(windows)]
    let job = {
        use std::os::windows::io::AsRawHandle;
        let j = job::Job::new();
        if let Some(j) = &j {
            j.assign(child.as_raw_handle());
        }
        j
    };
    let pid = child.id();
    let out = child.stdout.take().map(|o| pump(o, sink.clone(), false));
    let err = child.stderr.take().map(|e| pump(e, sink.clone(), true));
    std::thread::spawn(move || {
        let status = child.wait();
        // 等两个读取线程把剩余输出交完，保证"退出"是最后一个事件
        if let Some(h) = out {
            let _ = h.join();
        }
        if let Some(h) = err {
            let _ = h.join();
        }
        sink(Event::Exit(status.ok().and_then(|s| s.code())));
    });
    Ok(Running {
        pid,
        #[cfg(windows)]
        job,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    fn collect(spec: Spec) -> (String, String, Option<i32>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let e2 = events.clone();
        spawn(&spec, Arc::new(move |e| e2.lock().unwrap().push(e))).unwrap();
        let start = Instant::now();
        loop {
            if events
                .lock()
                .unwrap()
                .iter()
                .any(|e| matches!(e, Event::Exit(_)))
            {
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(20));
            std::thread::sleep(Duration::from_millis(10));
        }
        let ev = events.lock().unwrap();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut code = None;
        for e in ev.iter() {
            match e {
                Event::Stdout(d) => out.extend_from_slice(d),
                Event::Stderr(d) => err.extend_from_slice(d),
                Event::Exit(c) => code = *c,
            }
        }
        assert!(matches!(ev.last(), Some(Event::Exit(_))));
        (
            String::from_utf8_lossy(&out).into_owned(),
            String::from_utf8_lossy(&err).into_owned(),
            code,
        )
    }

    #[cfg(unix)]
    #[test]
    fn stdout_stderr_exit_code() {
        let (o, e, c) = collect(Spec {
            program: "sh".into(),
            args: vec!["-c".into(), "echo out; echo err >&2; exit 3".into()],
            ..Default::default()
        });
        assert_eq!(o.trim(), "out");
        assert_eq!(e.trim(), "err");
        assert_eq!(c, Some(3));
    }

    #[cfg(windows)]
    #[test]
    fn stdout_stderr_exit_code() {
        let (o, e, c) = collect(Spec {
            program: "cmd.exe".into(),
            // 每段不含空格：含空格的参数会被整体加引号，cmd 遇到引号内的 & 会把整串当成命令名
            args: "/d /c echo out& echo err 1>&2& exit /b 3"
                .split(' ')
                .map(String::from)
                .collect(),
            ..Default::default()
        });
        assert_eq!(o.trim(), "out");
        assert_eq!(e.trim(), "err");
        assert_eq!(c, Some(3));
    }

    #[test]
    fn missing_program_is_error() {
        let r = spawn(
            &Spec {
                program: "/no/such/program-hn".into(),
                ..Default::default()
            },
            Arc::new(|_| {}),
        );
        assert!(r.is_err());
    }
}
