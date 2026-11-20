//! Background work. Every job runs on its own thread and reports back through
//! a channel; the UI drains the channel once per frame and never blocks.

use std::sync::mpsc::{Receiver, Sender, channel};

use eframe::egui;
use git2::Oid;

use crate::forge::{ForgeError, ForgeSnapshot};
use crate::git::blame::{Blame, FileCommit};
use crate::git::conflict::MergeDoc;
use crate::git::ops::{GitOp, Outcome};
use crate::git::rebase::Plan;
use crate::git::repo::{CommitDetail, FileDiff, RepoSnapshot, WorkingTree};

pub enum JobResult {
    Snapshot {
        generation: u64,
        result: anyhow::Result<RepoSnapshot>,
    },
    CommitDetail {
        id: Oid,
        result: anyhow::Result<CommitDetail>,
    },
    Status {
        generation: u64,
        result: anyhow::Result<WorkingTree>,
    },
    FileDiff {
        path: String,
        staged: bool,
        result: anyhow::Result<FileDiff>,
    },
    Forge {
        generation: u64,
        result: Result<ForgeSnapshot, ForgeError>,
    },
    GitOp {
        op: GitOp,
        result: anyhow::Result<Outcome>,
    },
    PrCheckout {
        number: u64,
        result: Result<String, ForgeError>,
    },
    /// A conflicted file, parsed for the merge view.
    Conflict(MergeDoc),
    RebasePlan {
        from: Oid,
        result: anyhow::Result<Plan>,
    },
    FileHistory {
        path: String,
        result: anyhow::Result<(Vec<FileCommit>, bool)>,
    },
    Blame {
        path: String,
        rev: Option<Oid>,
        result: anyhow::Result<Blame>,
    },
    /// How one commit of a file's history changed the file.
    FileCommitDiff {
        id: Oid,
        result: anyhow::Result<FileDiff>,
    },
}

pub struct Jobs {
    tx: Sender<JobResult>,
    rx: Receiver<JobResult>,
    ctx: egui::Context,
}

impl Jobs {
    pub fn new(ctx: egui::Context) -> Self {
        let (tx, rx) = channel();
        Self { tx, rx, ctx }
    }

    pub fn spawn(&self, job: impl FnOnce() -> JobResult + Send + 'static) {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let result = job();
            // The receiver only disappears when the app is closing.
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    pub fn try_recv(&self) -> Option<JobResult> {
        self.rx.try_recv().ok()
    }
}
