clean-deps:
    cargo upgrade -i
    cargo machete

publish:
    cargo publish --package reva
    cargo publish --package reva_escape
    cargo publish --package reva_derive
    cargo publish --package reva_parser
    cargo publish --package reva_actix
    cargo publish --package reva_axum
