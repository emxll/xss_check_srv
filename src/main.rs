use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    ops::{Deref, DerefMut},
    sync::{Arc, Mutex},
    task::Waker,
};

use anyhow::{anyhow, Error};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use axum_macros::debug_handler;
use serde::Deserialize;
use tokio::task;

const MAX_FUTURES: usize = 10000;

struct AppError(anyhow::Error);

// Tell axum how to convert `AppError` into a response.
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Something went wrong: {}", self.0),
        )
            .into_response()
    }
}

impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self(err.into())
    }
}

struct ReqPoll {
    data: Arc<Mutex<Option<Result<HashMap<String, String>, Error>>>>,
    waker: Arc<Mutex<Option<Waker>>>,
}

impl ReqPoll {
    pub fn new() -> ReqPoll {
        ReqPoll {
            data: Arc::new(Mutex::new(None)),
            waker: Arc::new(Mutex::new(None)),
        }
    }
    pub fn fulfill(&self, data: Result<HashMap<String, String>, Error>) {
        *self.data.lock().expect("") = Some(data);
        let waker = self.waker.lock().expect("");
        let Some(waker) = waker.as_ref() else {
            return;
        };
        waker.wake_by_ref();
    }
}

impl Future for &ReqPoll {
    type Output = Result<HashMap<String, String>, Error>;
    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let mut data = self.data.lock().expect("");
        match std::mem::take(data.deref_mut()) {
            Some(res) => std::task::Poll::Ready(res),
            None => {
                *self.waker.lock().expect("") = Some(cx.waker().clone());
                std::task::Poll::Pending
            }
        }
    }
}

type Futures = Arc<Mutex<VecDeque<(String, Arc<ReqPoll>)>>>;

#[tokio::main]
async fn main() {
    let futures: Futures = Arc::new(Mutex::new(VecDeque::new()));

    let app = Router::new()
        .route("/notify", get(notify))
        .route("/poll-notified", get(poll_notified))
        .with_state(futures);
    let server =
        axum::Server::bind(&"127.0.0.1:3000".parse().unwrap()).serve(app.into_make_service());
    println!("Listening on 127.0.0.1:3000");
    server.await.unwrap();
}

async fn notify(
    Query(mut params): Query<HashMap<String, String>>,
    State(futures): State<Futures>,
) -> StatusCode {
    let Some(token) = params.remove("token") else {
        return StatusCode::BAD_REQUEST;
    };
    let suspended = {
        let mut guard = futures.lock().expect("");
        let suspended: Vec<_> = guard
            .deref()
            .into_iter()
            .filter(|entry| entry.0 == token)
            .map(|(_, r)| r.clone())
            .collect();
        guard.deref_mut().retain(|(_token, _)| *_token != token);
        suspended
    };
    task::spawn(async move {
        for r in suspended {
            r.fulfill(Ok(params.clone()))
        }
    });
    StatusCode::OK
}

#[derive(Deserialize)]
struct NotifyWait {
    token: String,
}

#[debug_handler]
async fn poll_notified(
    Query(NotifyWait { token }): Query<NotifyWait>,
    State(futures): State<Futures>,
) -> (StatusCode, Result<String, AppError>) {
    //FIXME: Limit futures
    let p = Arc::new(ReqPoll::new());
    {
        let mut guard = futures.lock().expect("");
        if guard.deref().len() > MAX_FUTURES {
            guard
                .deref_mut()
                .pop_front()
                .unwrap()
                .1
                .fulfill(Err(anyhow!("You got kicked")))
        }
        guard.deref_mut().push_back((token, p.clone()));
    }
    let data = p.as_ref().await;
    let Ok(data) = data else {
        return (
            StatusCode::REQUEST_TIMEOUT,
            Err(AppError(data.err().unwrap())),
        );
    };
    (
        StatusCode::OK,
        serde_json::to_string(&data).map_err(|e| AppError(anyhow!(e.to_string()))),
    )
}
