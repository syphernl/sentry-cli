//! Implements a command for managing projects.
use std::process;
use std::sync::Arc;
use std::time::Instant;

use clap::{App, AppSettings, Arg, ArgMatches};
use failure::{Error, ResultExt};
use uuid::Uuid;

use crate::api::{Api, CreateMonitorCheckIn, MonitorStatus, UpdateMonitorCheckIn};
use crate::config::Config;
use crate::utils::args::ArgExt;
use crate::utils::formatting::Table;
use crate::utils::system::QuietExit;

struct MonitorContext {
    pub api: Arc<Api>,
    pub org: String,
}

impl<'a> MonitorContext {
    pub fn get_org(&'a self) -> Result<&str, Error> {
        Ok(&self.org)
    }
}

pub fn make_app(app: App) -> App {
    app.about("Manage monitors on Sentry.")
        .setting(AppSettings::SubcommandRequiredElseHelp)
        .setting(AppSettings::Hidden)
        .org_arg()
        .subcommand(App::new("list").about("List all monitors for an organization."))
        .subcommand(
            App::new("run")
                .about("Wraps a command")
                .arg(
                    Arg::new("monitor")
                        .about("The monitor ID")
                        .required(true)
                        .index(1),
                )
                .arg(
                    Arg::new("allow_failure")
                        .short('f')
                        .long("allow-failure")
                        .about("Run provided command even when Sentry reports an error."),
                )
                .arg(Arg::new("args").required(true).multiple(true).last(true)),
        )
}

pub fn execute(matches: &ArgMatches) -> Result<(), Error> {
    let config = Config::current();

    let ctx = MonitorContext {
        api: Api::current(),
        org: config.get_org(matches)?,
    };

    if let Some(sub_matches) = matches.subcommand_matches("list") {
        return execute_list(&ctx, sub_matches);
    }
    if let Some(sub_matches) = matches.subcommand_matches("run") {
        return execute_run(&ctx, sub_matches);
    }
    unreachable!();
}

fn execute_list(ctx: &MonitorContext, _matches: &ArgMatches) -> Result<(), Error> {
    let mut monitors = ctx.api.list_organization_monitors(ctx.get_org()?)?;
    monitors.sort_by_key(|p| (p.name.clone()));

    let mut table = Table::new();
    table.title_row().add("ID").add("Name").add("Status");

    for monitor in &monitors {
        table
            .add_row()
            .add(&monitor.id)
            .add(&monitor.name)
            .add(&monitor.status);
    }

    table.print();

    Ok(())
}

fn execute_run(ctx: &MonitorContext, matches: &ArgMatches) -> Result<(), Error> {
    let monitor = matches
        .value_of("monitor")
        .unwrap()
        .parse::<Uuid>()
        .context("invalid monitor ID")?;
    let allow_failure = matches.is_present("allow_failure");
    let args: Vec<_> = matches.values_of("args").unwrap().collect();

    let monitor_checkin = ctx.api.create_monitor_checkin(
        &monitor,
        &CreateMonitorCheckIn {
            status: MonitorStatus::InProgress,
        },
    );

    let started = Instant::now();
    let mut p = process::Command::new(args[0]);
    p.args(&args[1..]);
    let exit_status = p.status()?;

    match monitor_checkin {
        Ok(checkin) => {
            ctx.api
                .update_monitor_checkin(
                    &monitor,
                    &checkin.id,
                    &UpdateMonitorCheckIn {
                        status: Some(if exit_status.success() {
                            MonitorStatus::Ok
                        } else {
                            MonitorStatus::Error
                        }),
                        duration: Some({
                            let elapsed = started.elapsed();
                            elapsed.as_secs() * 1000 + u64::from(elapsed.subsec_millis())
                        }),
                    },
                )
                .ok();
        }
        Err(e) => {
            if allow_failure {
                eprintln!("{}", e);
            } else {
                return Err(e.into());
            }
        }
    }

    if !exit_status.success() {
        if let Some(code) = exit_status.code() {
            Err(QuietExit(code).into())
        } else {
            Err(QuietExit(1).into())
        }
    } else {
        Ok(())
    }
}
