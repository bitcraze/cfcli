
# ---- cfcli dynamic completion -------------------------------------------
# Appended by build.rs to the clap-generated zsh completion. build.rs also
# rewrites the `_default` action of the param/log/flash-target arguments to
# call the helpers below. Each helper passes the whole current word to `cfcli
# __complete`, which handles comma-separated lists (returning fully-qualified
# tokens) and reads only a local cache (never connects).
# $1 = completion kind; $2 = optional suffix appended instead of a trailing
# space. For `name=value` contexts we pass '=' so completing leaves the cursor
# right after the '=', ready for the value.
_cfcli__dyn() {
    local cur="${words[CURRENT]}"
    local -a cands
    cands=(${(f)"$(cfcli __complete "$1" "$cur" 2>/dev/null)"})
    (( ${#cands} )) || return
    if [[ -n "$2" ]]; then
        compadd -U -Q -S "$2" -- $cands
    else
        compadd -U -Q -- $cands
    fi
}
_cfcli_param_names()   { _cfcli__dyn param-names }
_cfcli_param_set()     { _cfcli__dyn param-names-writable '=' }
_cfcli_config_set()    { _cfcli__dyn config-keys '=' }
_cfcli_log_names()     { _cfcli__dyn log-names }
_cfcli_flash_targets() { _cfcli__dyn flash-targets }
# `--bin` is a comma-separated list of `target=file`. In the current segment
# (after the last comma) complete the file path once past '=', otherwise the
# target name (appending '='). `compset -P '*='` moves the `…target=` prefix
# into IPREFIX so _files completes just the path portion.
_cfcli_flash_bin() {
    if [[ "${words[CURRENT]##*,}" == *=* ]]; then
        compset -P '*='
        _files
    else
        _cfcli__dyn flash-targets '='
    fi
}
