FROM rust:1.81

WORKDIR /app

COPY . .
RUN cargo build -p test-services
RUN cp ./target/debug/test-services /bin/server

ENV RUST_LOG="debug,restate_shared_core=trace"

CMD ["/bin/server"]