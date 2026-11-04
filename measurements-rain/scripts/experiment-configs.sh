# Shared three-configuration selection; sourced by experiment runners.
experiment_modes() {
    case $(( ($1 - 1) % 3 )) in
        0) modes=(any_unordered current_unordered current_ordered) ;;
        1) modes=(current_unordered current_ordered any_unordered) ;;
        2) modes=(current_ordered any_unordered current_unordered) ;;
    esac
}
experiment_variant() {
    case "$1" in
        any_unordered) variant="any::unordered::fixed::mmb" ;;
        any_unordered_btree) variant="any::unordered_btree::fixed::mmb" ;;
        current_unordered) variant="current::unordered::fixed::mmb chunk=32" ;;
        current_unordered_btree) variant="current::unordered_btree::fixed::mmb chunk=32" ;;
        current_ordered) variant="current::ordered::fixed::mmb chunk=32" ;;
        *) echo "unknown configuration: $1" >&2; return 1 ;;
    esac
}
