---
id: fnd_cf4bf367-4acc-467f-a41f-d84981f8d4ee
tags:
- directive
- polarity:do
parent_ids: []
created_at: 2026-08-16T09:55:32.263762Z
---
A test guarding a POLICY choice must contain the case the policy discriminates against, or it passes without ever reaching the branch. Three false guards in one session, all this shape: (1) a find_symbol fixture with ONE symbol, where the 4th n-gram tier returns that symbol for any query, so asserting a name match proved nothing; (2) a test for 'a qualified reference does not absorb a bare identity' built on an EMPTY registry, so no bare identity existed and the adoption branch never ran — it passed while the code merged sales.orders into archive.orders on a real index; (3) one fixture covering two independent fixes, where the first fix alone made it pass, so the second fix's mutation survived. The rule: name the alternative the policy rejects, and put it in the fixture. Then mutate EACH fix separately — a mutation that survives means the fixture, not the assertion, is wrong. All three were caught by mutation and none by review.