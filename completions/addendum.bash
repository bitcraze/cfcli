
# ---- cfcli dynamic completion -------------------------------------------
# Appended by build.rs to the clap-generated bash completion. Adds param/log
# variable names and flash targets sourced from `cfcli __complete`, which
# reads only a local cache (written on each connection) and never talks to a
# Crazyflie. Wrapping the generated `_cfcli` keeps this robust across clap
# versions: we let clap complete first, then add our candidates.
#
# Note: bash's COMP_WORDBREAKS contains ',' and '=', so $cur is already the
# fragment after the last comma/equals, which is exactly what `cfcli
# __complete` expects.
_cfcli_dynamic() {
    # Forward the (cmd, cur, prev) positional args: clap's _cfcli reads cur/prev
    # from $2/$3 on bash >= 4, so calling it bare breaks command completion.
    _cfcli "$@"

    # Drop clap's positional metavar placeholders (e.g. [PARAMS], <NAME>) and
    # the hidden __complete helper — none of these are real completions.
    local _c _kept=()
    for _c in "${COMPREPLY[@]}"; do
        case "$_c" in
            '['*']' | '<'*'>' | __complete) ;;
            *) _kept+=("$_c") ;;
        esac
    done
    COMPREPLY=("${_kept[@]}")

    local cur prev kind line suffix nospace
    cur="${COMP_WORDS[COMP_CWORD]}"
    prev="${COMP_WORDS[COMP_CWORD-1]}"
    line=" ${COMP_WORDS[*]} "

    # suffix='=' : append '=' on a single match (param set `name=value`, flash
    #   `--bin target=file`); implies nospace.
    # nospace=1  : just suppress the trailing space, no suffix — for
    #   comma-separated lists (log print) so the next ',' can be typed.
    kind=""
    suffix=""
    nospace=""
    case "$line" in
        *" param set "*)                              kind="param-names-writable"; suffix="="; nospace=1 ;;
        *" param get "*|*" param store "*|*" param clear "*) kind="param-names" ;;
        *" config set "*)                             kind="config-keys"; suffix="="; nospace=1 ;;
        *" log print "*)                              kind="log-names"; nospace=1 ;;
    esac

    # Option values: `--targets x,y` (plain list) / `--bin t=f` (key=value),
    # space- or '='-separated.
    case "$prev" in
        --bin)     kind="flash-targets"; suffix="="; nospace=1 ;;
        --targets) kind="flash-targets" ;;
        =) case "${COMP_WORDS[COMP_CWORD-2]}" in
               --bin)     kind="flash-targets"; suffix="="; nospace=1 ;;
               --targets) kind="flash-targets" ;;
           esac ;;
    esac

    if [[ -n "$kind" ]]; then
        local cands=() c
        while IFS= read -r c; do
            [[ -n "$c" ]] && cands+=("$c")
        done < <(cfcli __complete "$kind" "$cur" 2>/dev/null)

        if (( ${#cands[@]} > 0 )); then
            # Append the suffix only once it resolves to a single match (that's
            # the actual insertion); with multiple matches list bare names
            # (COMPREPLY drives both menu and insertion in bash).
            if [[ -n "$suffix" ]] && (( ${#cands[@]} == 1 )); then
                COMPREPLY+=("${cands[0]}${suffix}")
            else
                COMPREPLY+=("${cands[@]}")
            fi
            [[ -n "$nospace" ]] && compopt -o nospace
        fi
    fi
}
complete -F _cfcli_dynamic -o bashdefault -o default cfcli
