    # ---- cfcli dynamic completion ----------------------------------------
    # Injected by build.rs before the final filter of the clap-generated
    # completer. Adds param/log variable names and flash targets from
    # `cfcli __complete` (local cache only; never connects). $command is the
    # ';'-joined subcommand path; $completions is the static list built above.
    # $cfcliSuffix is appended to each candidate ('=' for `name=value`
    # contexts: param set and flash `--bin target=file`), so completion leaves
    # the cursor right after the '=' ready for the value.
    $cfcliDynKind = ''
    $cfcliSuffix = ''
    switch -regex ($command) {
        'cfcli;param;set$'             { $cfcliDynKind = 'param-names-writable'; $cfcliSuffix = '=' }
        'cfcli;param;(get|store|clear)$' { $cfcliDynKind = 'param-names' }
        'cfcli;config;set$'            { $cfcliDynKind = 'config-keys'; $cfcliSuffix = '=' }
        'cfcli;log;print$'             { $cfcliDynKind = 'log-names' }
    }
    # Option values: `--targets x,y` (plain list) / `--bin t=f` (key=value),
    # space-separated form.
    $cfcliPrev = ''
    if ($commandElements.Count -ge 1) {
        $cfcliLast = $commandElements[$commandElements.Count - 1].ToString()
        if ($cfcliLast -eq $wordToComplete -and $commandElements.Count -ge 2) {
            $cfcliPrev = $commandElements[$commandElements.Count - 2].ToString()
        } elseif ($cfcliLast -ne $wordToComplete) {
            $cfcliPrev = $cfcliLast
        }
    }
    if ($cfcliPrev -eq '--targets') { $cfcliDynKind = 'flash-targets' }
    if ($cfcliPrev -eq '--bin')     { $cfcliDynKind = 'flash-targets'; $cfcliSuffix = '=' }

    if ($cfcliDynKind -ne '') {
        # `cfcli __complete` handles comma-separated lists, returning fully
        # qualified tokens that already start with $wordToComplete, so the
        # filter below keeps them.
        & cfcli __complete $cfcliDynKind $wordToComplete 2>$null | ForEach-Object {
            $cfcliText = "$_$cfcliSuffix"
            $completions += [CompletionResult]::new($cfcliText, $_, [CompletionResultType]::ParameterValue, $cfcliText)
        }
    }

