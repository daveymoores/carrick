# `parallel-services`

Four services in one npm workspace, declared largest first, for the tests of a
scan that analyses services side by side (`tests/parallel_services_test.rs`):

- `orders-api`: an HTTP API with routers in several files and typed responses.
- `catalog-api`: a small HTTP API.
- `storefront-web`: a client calling both through env-var base URLs, with
  typed reads.
- `notifier`: the smallest, a service that publishes and subscribes events.

`orders-api` and `catalog-api` each define the same helper (`formatCents`,
same name and body), so the run's intent memo is shared between services.

`__llm__/` holds the model's answer for every file it is asked about.
