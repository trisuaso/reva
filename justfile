clean-deps:
    cargo upgrade -i
    cargo machete

publish:
    just publish-core
    just publish-extras

publish-core:
    cargo publish --package reva_parser
    cargo publish --package reva_escape
    cargo publish --package reva_derive
    cargo publish --package reva

publish-extras:
    echo "Publishing more in 15 seconds..."
    sleep 15
    cargo publish --package reva_actix
    cargo publish --package reva_axum
