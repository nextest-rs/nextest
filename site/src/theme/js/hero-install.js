// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// Pre-select the Windows tab on Windows; a tab the reader already picked
// (restored by Material from __tabs) wins over detection.
document$.subscribe(function () {
    var set = document.querySelector(".mdx-hero__install");
    if (!set) {
        return;
    }

    var labels = Array.prototype.map.call(
        set.querySelectorAll(".tabbed-labels > label"),
        function (label) {
            return label.innerText.trim();
        },
    );

    // Material only records choices for tab sets inside the page content, and
    // the hero is outside it. So record the reader's choice the same way so it
    // persists and carries over to the install page.
    Array.prototype.forEach.call(set.querySelectorAll(":scope > input"), function (input) {
        input.addEventListener("change", function () {
            var label = set.querySelector('label[for="' + input.id + '"]');
            var tab = label.innerText.trim();
            var previous = __md_get("__tabs") || [];
            __md_set(
                "__tabs",
                [tab].concat(
                    previous.filter(function (other) {
                        return other !== tab;
                    }),
                ),
            );
        });
    });

    var saved = __md_get("__tabs");
    if (
        Array.isArray(saved) &&
        saved.some(function (tab) {
            return labels.indexOf(tab) !== -1;
        })
    ) {
        return;
    }

    var platform =
        (navigator.userAgentData && navigator.userAgentData.platform) ||
        navigator.platform ||
        "";
    if (/^win/i.test(platform)) {
        set.querySelector('input[data-hero-os="windows"]').checked = true;
    }
});
