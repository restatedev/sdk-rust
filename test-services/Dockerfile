FROM rust:1.85

WORKDIR /app

COPY . .
RUN cargo build -p test-services
RUN cp ./target/debug/test-services /bin/server

ENV RUST_LOG="debug,restate_shared_core=trace"
ENV RUST_BACKTRACE=1

CMD ["/bin/server"]