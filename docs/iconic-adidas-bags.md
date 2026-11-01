# THE ICONIC adidas bag alerts

The checked-in workflow at `src/iconic/iconic-adidas-bags-workflow.json` searches
THE ICONIC men's bags using `q=adidas&sort=new`, without restricting brand facets.
It runs daily at 07:00 Australia/Brisbane through the existing Browserless service.
The Browserless function is maintained in `scripts/iconic-adidas-browserless.js`
and embedded in the workflow JSON; keep both copies synchronized.

All result pages are checked (up to 20; reaching the limit fails visibly).
Men's and unisex adidas products matching crossbody, messenger, or shoulder in
THE ICONIC's category or product title are included. Backpacks, totes and waist
bags without those classifications are excluded. Currently the matching results
happen to be Originals products; Performance and future adidas brands are not
excluded. Classification depends on retailer metadata and names.

Messages include a public product thumbnail, title, brand, AUD price, and retailer
link. Prices are the retailer's listed final price; conditional offers may differ.
The Matrix service approves only `static.theiconic.com.au` for these images.
The existing encrypted Matrix Sender resizes them to fit 256 × 256 pixels.

The `THE ICONIC adidas bags` data table records product ID and delivery metadata
only after Matrix delivery succeeds. Stable Matrix request IDs protect retries.
A new colour listing is a separate product ID. Price changes do not resend alerts.
Previously existing products are eligible on the first scheduled run.

The manual `Preview one bag` entry point takes the first matching result and
passes through the same deduplication and delivery path. Run on demand accepts
`preview: true` for that same behavior. A preview records successful delivery,
so that item will not be repeated by the schedule.

Verified live: the keyword search traversed two pages and found three matching
listings. Preview execution 18887 delivered an encrypted image and text, with
monitor verification, then recorded the product. Repeat execution 18889 succeeded
without invoking Matrix delivery. The temporary test webhook was removed.
