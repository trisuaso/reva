# Integrations

## Rocket integration

In your template definitions, replace `reva::Template` with
[`reva_rocket::Template`][reva_rocket].

Enabling the `with-rocket` feature appends an implementation of Rocket's
`Responder` trait for each template type. This makes it easy to trivially
return a value of that type in a Rocket handler. See
[the example](https://github.com/trisuaso/reva/blob/main/reva_rocket/tests/basic.rs)
from the Reva test suite for more on how to integrate.

In case a run-time error occurs during templating, a `500 Internal Server
Error` `Status` value will be returned, so that this can be further
handled by your error catcher.

## Actix-web integration

In your template definitions, replace `reva::Template` with
[`reva_actix::Template`][reva_actix].

Enabling the `with-actix-web` feature appends an implementation of Actix-web's
`Responder` trait for each template type. This makes it easy to trivially return
a value of that type in an Actix-web handler. See
[the example](https://github.com/trisuaso/reva/blob/main/reva_actix/tests/basic.rs)
from the Reva test suite for more on how to integrate.

## Axum integration

In your template definitions, replace `reva::Template` with
[`reva_axum::Template`][reva_axum].

Enabling the `with-axum` feature appends an implementation of Axum's
`IntoResponse` trait for each template type. This makes it easy to trivially
return a value of that type in a Axum handler. See
[the example](https://github.com/trisuaso/reva/blob/main/reva_axum/tests/basic.rs)
from the Reva test suite for more on how to integrate.

In case of a run-time error occurring during templating, the response will be of the same
signature, with a status code of `500 Internal Server Error`, mime `*/*`, and an empty `Body`.
This preserves the response chain if any custom error handling needs to occur.

## Warp integration

In your template definitions, replace `reva::Template` with
[`reva_warp::Template`][reva_warp].

Enabling the `with-warp` feature appends an implementation of Warp's `Reply`
trait for each template type. This makes it simple to return a template from
a Warp filter. See [the example](https://github.com/trisuaso/reva/blob/main/reva_warp/tests/warp.rs)
from the Reva test suite for more on how to integrate.

[reva_rocket]: https://docs.rs/reva_rocket
[reva_actix]: https://docs.rs/reva_actix
[reva_axum]: https://docs.rs/reva_axum
[reva_warp]: https://docs.rs/reva_warp
