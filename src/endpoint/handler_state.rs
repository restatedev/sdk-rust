use crate::endpoint::Error;
use tokio::sync::oneshot;

pub(super) struct HandlerStateNotifier {
    tx: Option<oneshot::Sender<Error>>,
}

impl HandlerStateNotifier {
    pub(crate) fn new() -> (Self, oneshot::Receiver<Error>) {
        let (tx, rx) = oneshot::channel();
        (Self { tx: Some(tx) }, rx)
    }

    pub(super) fn mark_error(&mut self, err: Error) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(err);
        }
        // Some other operation already marked this handler as errored.
    }
}
