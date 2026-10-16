
# Memory names are inserted by build.rs; this completion never connects.
function __cfcli_needs_memory_type
    set -l tokens (commandline -opc)
    set -e tokens[1]
    argparse -s (__fish_cfcli_global_optspecs) -- $tokens 2>/dev/null
    or return 1

    test (count $argv) -ge 2
    or return 1
    test "$argv[1]" = mem
    or return 1
    contains -- $argv[2] display read write verify erase
    or return 1
    set -e argv[1..2]

    argparse s/offset= n/length= o/output= d/data= i/input= timeout= non-interactive csv h/help -- $argv 2>/dev/null
    or return 1
    test (count $argv) -eq 0
end

complete -c cfcli -n '__cfcli_needs_memory_type' -f -a '@CFCLI_MEMORY_TYPES@'
