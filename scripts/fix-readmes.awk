# Fix up readmes:
# * Replace ## with # in code blocks.
# * Remove [] without a following () from output.

BEGIN {
    true = 1
    false = 0
    in_block = false
}

{
    if (!in_block && $0 ~ /^```/) {
        in_block = true
    } else if (in_block && $0 ~ /^```$/) {
        in_block = false
    }

    if (in_block) {
        sub(/## /, "# ")
        print $0
    } else {
        # Strip [] without a () that immediately follows them from
        # the output.
        subbed = gensub(/\[([^\[]+)]([^\(]|$)/, "\\1\\2", "g")
        print subbed
    }
}
