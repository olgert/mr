use clap::Arg;
use std::fmt;
use std::thread;
use std::time::Duration;
use std::process::Command;
use std::borrow::Cow;
use std::time::Instant;
use std::cmp;
use std::process::ExitStatus;


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
    let influxdb = InfluxDBOptions{
        host: matches.value_of("influxdb-host"),
        port: matches.value_of("influxdb-port"),
        username: matches.value_of("influxdb-username"),
        password: matches.value_of("influxdb-password"),
        dbname: matches.value_of("influxdb-dbname"),
        rpname: matches.value_of("influxdb-rpname")
    };
    let artifacts = ArtifactsOptions{
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
    let max_wait_step = Duration::from_millis(500);

    println!("Runtime options: {}", runtime);

    loop {
        let start = Instant::now();
        let terminate_at = start + timeout;
        let mut split = runtime.test_cmd.split_whitespace();
        let mut cmd = Command::new(split.next().expect("Can't parse test_cmd"));
        let mut child = cmd.args(split).spawn().expect("Command failed");
        let mut delay = Duration::from_micros(500);
        let mut exit_code = None;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    println!("exited with: {}", status);
                    exit_code = status.code();
                    break;
                },
                Ok(None) => {
                    if Instant::now() > terminate_at {
                        let result = child.kill();
                        println!("Attempted to kill the monitor: {:?}", result);
                        result.expect("Error killing the monitor");
                        exit_code = Some(128 + 9);
                        break;
                    }
                    delay *= 2;
                    let remaining = interval - (Instant::now() - start);
                    let sleep_for = cmp::min(cmp::min(delay, max_wait_step), remaining);
                    println!("waiting for {}ms ...", sleep_for.as_millis());
                    thread::sleep(sleep_for);
                }
                Err(e) => {
                    println!("error attempting to wait: {}", e);
                    break;
                },
            }
        }
        let duration = Instant::now() - start;
        println!("Run took {}ms", duration.as_millis());
        println!("Status: {:?}", exit_code);
        println!("cm,app={},name={},ret_code={} value=1,duration={},interval={},routing_key={},artifact_url={},image_url={}",
                 runtime.app_name, runtime.name, exit_code.unwrap_or(-1),
                 duration.as_millis(), runtime.interval, routing_key, "<artifact_url>", "<image_url>");
        if interval > duration {
            thread::sleep(interval - duration);
        }
    }

}

impl<'a> fmt::Display for RuntimeOptions<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "({}/{}: {} every {}s for {}s)",
               self.app_name,
               self.name,
               self.test_cmd,
               self.interval,
               self.timeout)
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

struct InfluxDBOptions<'a> {
    host: Option<&'a str>,
    port: Option<&'a str>,
    username: Option<&'a str>,
    password: Option<&'a str>,
    dbname: Option<&'a str>,
    rpname: Option<&'a str>,
}

struct ArtifactsOptions<'a> {
    artifacts_glob: Option<&'a str>,
    image_artifact: Option<&'a str>,
    aws_access_key: Option<&'a str>,
    aws_secret_access_key: Option<&'a str>,
}