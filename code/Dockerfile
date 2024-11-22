ARG APP_HOME="/opt/reciper"
ARG RUST_VERSION=1.82.0
FROM rust:${RUST_VERSION}-bookworm AS builder
ARG APP_HOME
ENV APP_HOME=${APP_HOME}
WORKDIR ${APP_HOME}
COPY . .
RUN --mount=type=cache,target=/home/noojee/bmstu_aa/2024/microservices/target \
    --mount=type=cache,target=/home/noojee/.cargo/git/db \
    --mount=type=cache,target=/home/noojee/.cargo/registry \
    cargo build --release
RUN ["bash"]

FROM debian:bookworm AS load_automaton
RUN apt-get update -qq -y && \
    apt-get install -qq -y --no-install-recommends libssl3 ca-certificates && \
    rm -rf /var/lib/apt/lists/* /tmp/*
COPY --from=builder /opt/reciper/target/release/load_automaton /usr/local/bin
COPY .env ./
COPY tests ./tests
CMD [ "load_automaton" ]
LABEL service=load_automaton

FROM debian:bookworm AS parse_automaton
RUN apt-get update -qq -y && \
    apt-get install -qq -y --no-install-recommends libssl3 ca-certificates curl && \
    rm -rf /var/lib/apt/lists/* /tmp/*
COPY --from=builder /opt/reciper/target/release/parse_automaton /usr/local/bin
COPY .env ./
CMD [ "parse_automaton" ]
LABEL service=parse_automaton

FROM debian:bookworm AS store_automaton
RUN apt-get update -qq -y && \
    apt-get install -qq -y --no-install-recommends libssl3 ca-certificates curl && \
    rm -rf /var/lib/apt/lists/* /tmp/*
COPY --from=builder /opt/reciper/target/release/store_automaton /usr/local/bin
COPY .env ./
CMD [ "store_automaton" ]
LABEL service=store_automaton

FROM debian:bookworm AS web_server
RUN apt-get update -qq -y && \
    apt-get install -qq -y --no-install-recommends libssl3 ca-certificates curl && \
    rm -rf /var/lib/apt/lists/* /tmp/*
COPY --from=builder /opt/reciper/target/release/web_server /usr/local/bin
COPY .env ./
CMD [ "web_server" ]
LABEL service=web_server