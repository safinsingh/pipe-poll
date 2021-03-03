/*
 * pipe-poll
 * Copyright (c) 2021 Safin Singh
 *
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with this program.  If not, see <https://www.gnu.org/licenses/>.
*/

use anyhow::{Context as _, Result};
use async_std::task;
use libc::{
	epoll_create1, epoll_ctl, epoll_event, epoll_wait, EPOLLIN, EPOLL_CTL_ADD,
	O_CLOEXEC,
};
use std::{
	env,
	fs::File,
	future::Future,
	io::Read,
	os::unix::prelude::AsRawFd,
	pin::Pin,
	process,
	sync::{Arc, Mutex},
	task::{Context, Poll, Waker},
	thread,
	time::Duration,
};

const MAX_EVENTS: i32 = 1;
const HELP: &str = r#"
USAGE:
	pipe-poll <pipe>

ARGUMENTS:
	pipe - location of named pipe

EXAMPLES:
	mkfifo pipe
	pipe-poll pipe
"#;

macro_rules! handle_errno {
	($ex:expr) => {{
		use std::io;

		let _res = unsafe { $ex };
		if _res == -1 {
			panic!("{:?}", io::Error::last_os_error())
		} else {
			_res
		}
	}};

	($($ex:expr),*) => {{
		$(
			handle_errno!($ex);
		)*
	}};
}

struct PipeWriteListenState {
	waker: Option<Waker>,
	file: File,
	complete: bool,
	content: Option<String>,
}

struct PipeWriteListen {
	state: Arc<Mutex<PipeWriteListenState>>,
}

impl Future for PipeWriteListen {
	type Output = String;

	fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
		let mut state = self.state.lock().unwrap();

		if state.complete {
			if let Some(ct) = state.content.take() {
				Poll::Ready(ct)
			} else {
				unreachable!()
			}
		} else {
			state.waker = Some(cx.waker().clone());
			Poll::Pending
		}
	}
}

impl PipeWriteListen {
	fn new(loc: &str) -> Self {
		let state = Arc::new(Mutex::new(PipeWriteListenState {
			waker: None,
			complete: false,
			file: File::open(loc).unwrap(),
			content: None,
		}));

		let thread_state = state.clone();
		thread::spawn(move || {
			let mut thread_state = thread_state.lock().unwrap();

			let epoll_fd = handle_errno!(epoll_create1(O_CLOEXEC));
			let mut event = epoll_event {
				events: EPOLLIN as u32,
				u64: 0,
			};

			handle_errno!(
				epoll_ctl(
					epoll_fd,
					EPOLL_CTL_ADD,
					thread_state.file.as_raw_fd(),
					&mut event
				),
				epoll_wait(epoll_fd, [event].as_mut_ptr(), MAX_EVENTS, -1)
			);

			let mut buf = String::new();

			thread_state.complete = true;
			thread_state.file.read_to_string(&mut buf).unwrap();

			// If this were an &'a str and PipeWriteListenState and therefore
			// PipeWriteListen would be constricted to the lifetime 'a.
			// This is an issue because `PipeWriteListen::new()` (which
			// would return PipeWriteListen bound to the 'a lifetime) spawns
			// a thread which captures a cloned arc referencing the variable
			// representing PipeWriteListenState<'a>. since that spawned thread
			// may live longer than `PipeWriteListen::new()`, if the closure
			// passed to `thread::spawn` was not bound to 'static, there
			// is a possibility of a use-after-free bug (since the thread
			// would be trying to reference memory the memory which was freed
			// when `PipeWriteListen::new()` went out of scope).
			thread_state.content = Some(buf.trim().to_string());

			if let Some(waker) = thread_state.waker.take() {
				waker.wake()
			}
		});

		PipeWriteListen { state }
	}
}

#[async_std::main]
async fn main() -> Result<()> {
	let pipe = env::args().nth(1).with_context(|| {
		format!(
			"Please pass the location of a shared pipe as argv[1]!{}",
			HELP
		)
	})?;

	if pipe == "-h" || pipe == "--help" {
		eprintln!("{}", HELP);
		process::exit(1);
	}

	task::spawn(async move {
		let written = PipeWriteListen::new(&pipe).await;
		println!("Thread finished its job with: {}!", written);
		process::exit(0);
	});

	let mut seconds = 0;
	loop {
		println!(
			"Waiting for the future to complete... it's been {} seconds",
			seconds
		);
		thread::sleep(Duration::from_secs(1));
		seconds += 1;
	}
}
