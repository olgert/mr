use std::fmt;
use std::thread;
use std::time::Duration;
use std::process::Command;
use std::time::Instant;
use clap::Arg;
use nix;
use nix::sys::signal;
use nix::unistd::Pid;
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus::StillAlive};
use nix::sys::signal::kill;


fn main() {
    let matches = clap::App::new("mr")
        .version("0.1")
        .author("DevOps")
        .about("Monitor Runner")
        .arg(Arg::with_name("test_cmd").takes_value(true)
            .env("MONITOR_TEST_CMD")
            .help("Shell command which runs the test one time (REQUIRED)"))
        .arg(Arg::with_name("app-name").takes_value(true)
            .long("app-name").env("MONITOR_APP_NAME")
            .help("Name of application under test (REQUIRED)"))
        .arg(Arg::with_name("name").takes_value(true)
            .long("name").env("MONITOR_NAME")
            .help("Name of test (REQUIRED)"))
        .arg(Arg::with_name("interval").takes_value(true)
            .long("interval").env("MONITOR_INTERVAL").default_value("10")
            .help("How often to run the test in seconds"))
        .arg(Arg::with_name("timeout").takes_value(true)
            .long("timeout").env("MONITOR_TIMEOUT").default_value("5")
            .help("Number of seconds to wait before killing test run"))
        .arg(Arg::with_name("influxdb-measurement").takes_value(true)
            .long("influxdb-measurement").env("MONITOR_INFLUXDB_MEASUREMENT").default_value("cm")
            .help("InfluxDB measurement name to write"))
        .arg(Arg::with_name("influxdb-host").takes_value(true)
            .long("influxdb-host").env("MONITOR_INFLUXDB_HOST")
            .help("InfluxDB host to send stats to"))
        .arg(Arg::with_name("influxdb-port").takes_value(true)
            .long("influxdb-port").env("MONITOR_INFLUXDB_PORT").default_value("8086")
            .help("InfluxDB port to send stats to"))
        .arg(Arg::with_name("influxdb-username").takes_value(true)
            .long("influxdb-username").env("MONITOR_INFLUXDB_USERNAME")
            .help("InfluxDB username"))
        .arg(Arg::with_name("influxdb-password").takes_value(true)
            .long("influxdb-password").env("MONITOR_INFLUXDB_PASSWORD")
            .help("InfluxDB password"))
        .arg(Arg::with_name("influxdb-dbname").takes_value(true)
            .long("influxdb-dbname").env("MONITOR_INFLUXDB_DBNAME").default_value("monitor")
            .help("InfluxDB database name to send stats to"))
        .arg(Arg::with_name("influxdb-rpname").takes_value(true)
            .long("influxdb-rpname").env("MONITOR_INFLUXDB_RPNAME")
            .help("InfluxDB retention policy name to send stats to"))
        .arg(Arg::with_name("routing-key").takes_value(true)
            .long("routing-key").env("MONITOR_ROUTING_KEY")
            .help("OpsGenie team name, passed to InfluxDB"))
        .arg(Arg::with_name("artifacts-glob").takes_value(true)
            .long("artifacts-glob").env("MONITOR_ARTIFACT_GLOB")
            .help("Artifacts matching glob pattern to archive on monitor failures"))
        .arg(Arg::with_name("image-artifact").takes_value(true)
            .long("image-artifact").env("MONITOR_IMAGE_PATH")
            .help("Path to image artifact to archive on monitor failures"))
        .arg(Arg::with_name("aws-access-key").takes_value(true)
            .long("aws-access-key").env("MONITOR_AWS_ACCESS_KEY_ID")
            .help("AWS_ACCESS_KEY_ID credential for S3 artifact archival"))
        .arg(Arg::with_name("aws-secret-access-key").takes_value(true)
            .long("aws-secret-access-key").env("MONITOR_AWS_SECRET_ACCESS_KEY")
            .help("AWS_SECRET_ACCESS_KEY credential for S3 artifact archival"))
        .get_matches();


    let runtime: RuntimeOptions = RuntimeOptions {
        app_name: matches.value_of("app-name").unwrap(),
        name: matches.value_of("name").unwrap(),
        test_cmd: matches.value_of("test_cmd").unwrap(),
        interval: matches.value_of("interval").unwrap().parse().unwrap(),
        timeout: matches.value_of("timeout").unwrap().parse().unwrap(),
    };
    let influxdb = InfluxDBOptions {
        measurement: matches.value_of("influxdb-measurement").unwrap(),
        host: matches.value_of("influxdb-host"),
        port: matches.value_of("influxdb-port"),
        username: matches.value_of("influxdb-username"),
        password: matches.value_of("influxdb-password"),
        dbname: matches.value_of("influxdb-dbname"),
        rpname: matches.value_of("influxdb-rpname")
    };
    let artifacts = ArtifactsOptions {
        artifacts_glob: matches.value_of("artifacts-glob"),
        image_artifact: matches.value_of("image-artifact"),
        aws_access_key: matches.value_of("aws-access-key"),
        aws_secret_access_key: matches.value_of("aws-secret-access-key")
    };
    schedule(runtime, influxdb, artifacts, "monitor-pilot");
    println!("Hello, world!");

}


fn schedule(runtime: RuntimeOptions,
            influxdb: InfluxDBOptions,
            artifacts: ArtifactsOptions,
            routing_key: &str) {
    let interval = Duration::from_secs(runtime.interval as u64);
    let timeout = Duration::from_secs(runtime.timeout as u64);
    if timeout > interval {
        panic!("Timeout has to be less than Interval")
    }

    println!("Runtime options: {}", runtime);
    println!("InfluxDB options: {}", influxdb);
    println!("Artifacts options: {}", artifacts);

    loop {
        let start = Instant::now();
        let mut split = runtime.test_cmd.split_whitespace();
        let mut cmd = Command::new(split.next().expect("Can't parse test_cmd"));
        let mut child = cmd.args(split).spawn().expect("Command failed");
        let child_pid = Pid::from_raw(child.id() as i32);
        let spwn = thread::Builder::new()
            .name("KillerThread".to_string())
            .spawn(move || killer_routine(child_pid, timeout));

        let result = child.wait();
        let exit_code = result.expect("unable to get result").code();
        let duration = Instant::now() - start;
        println!("Run took {}ms", duration.as_millis());
        println!("Status: {:?}", exit_code);
        println!("{},app={},name={},ret_code={} value=1,duration={},interval={},routing_key={},artifact_url={},image_url={}",
                 influxdb.measurement, runtime.app_name, runtime.name, exit_code.unwrap_or(-1),
                 duration.as_millis(), runtime.interval, routing_key, "<artifact_url>", "<image_url>");
        if interval > duration {
            thread::sleep(interval - duration);
        }
    }

}

fn killer_routine(pid: Pid, timeout: Duration) {
    thread::sleep(timeout);
    match waitpid(pid, Some(WaitPidFlag::WNOHANG)) {
        Ok(StillAlive) => {
            match kill(pid, signal::SIGKILL) {
                Ok(_) => {
                    println!("Killed monitor by timeout. PID: {}", pid);
                },
                Err(e) => {
                    println!("Error killing monitor by timeout. PID: {}, ERROR: {}", pid, e);
                }
            }
        },
        Ok(status) => {
            println!("Monitor is in status ({:?})", status);
        }
        Err(e) => {
            println!("Monitor is not running ({})", e);
        }
    }
}

struct RunResult {
    exit_code: u8,
    duration: Duration,
}

struct RuntimeOptions<'a> {
    app_name: &'a str,
    name: &'a str,
    test_cmd: &'a str,
    interval: u32,
    timeout: u32,
}

impl<'a> fmt::Display for RuntimeOptions<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Runtime[{}/{}: {} every {}s for {}s]",
               self.app_name,
               self.name,
               self.test_cmd,
               self.interval,
               self.timeout,
        )
    }
}

struct InfluxDBOptions<'a> {
    measurement: &'a str,
    host: Option<&'a str>,
    port: Option<&'a str>,
    username: Option<&'a str>,
    password: Option<&'a str>,
    dbname: Option<&'a str>,
    rpname: Option<&'a str>,
}

impl<'a> fmt::Display for InfluxDBOptions<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "InfluxDB[({}) to http://{}:{}@{}:{}/write?db={}&rp={}]",
               self.measurement,
               self.username.and(Some("*****")).unwrap_or_default(),
               self.password.and(Some("*****")).unwrap_or_default(),
               self.host.unwrap_or_default(),
               self.port.unwrap_or_default(),
               self.dbname.unwrap_or_default(),
               self.rpname.unwrap_or_default(),
        )
    }
}

struct ArtifactsOptions<'a> {
    artifacts_glob: Option<&'a str>,
    image_artifact: Option<&'a str>,
    aws_access_key: Option<&'a str>,
    aws_secret_access_key: Option<&'a str>,
}

impl<'a> fmt::Display for ArtifactsOptions<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Artifacts[{}/{} {}/{}]",
               self.artifacts_glob.unwrap_or_default(),
               self.image_artifact.unwrap_or_default(),
               self.aws_access_key.unwrap_or_default(),
               self.aws_secret_access_key.and(Some("*****")).unwrap_or_default(),
        )
    }
}