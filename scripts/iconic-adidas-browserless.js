export default async function ({ page }) {
  const products = new Map();
  for (let number = 1; number <= 20; number++) {
    const response = await page.goto(`https://www.theiconic.com.au/mens-accessories-bags/?q=adidas&sort=new&page=${number}`, { waitUntil: 'networkidle2', timeout: 55000 });
    if (!response.ok()) throw new Error(`THE ICONIC returned HTTP ${response.status()}`);
    const result = await page.evaluate(() => {
      const html = document.documentElement.outerHTML;
      if (!document.querySelector('#catalogProductsList') || !document.querySelector('#sort-newarrivals:checked')) throw new Error('Missing catalogue or newest-first sort');
      let metadata;
      for (const script of document.scripts) {
        for (const match of script.textContent.matchAll(/dl\.push\((\{[^\n]+\})\);/g)) {
          try { const data = JSON.parse(match[1]); if (data.event === 'ti.data.products' && data.eventData?.list === 'catalog') metadata = data.products; } catch {}
        }
      }
      if (!metadata) throw new Error('Missing product metadata');
      const found = [];
      for (const tile of document.querySelectorAll('#catalogProductsList [data-ti-track-product]')) {
        const id = tile.getAttribute('data-ti-track-product');
        const product = metadata[id];
        if (!product) throw new Error(`Missing metadata for ${id}`);
        if (!/\badidas\b/i.test(product.brand) || !['male', 'unisex'].includes(product.gender)) continue;
        if (!/cross[ -]?body|messenger|shoulder/i.test(`${product.category} ${product.name}`)) continue;
        const link = tile.querySelector('a.product-image-link');
        const image = tile.querySelector('.image-frame img');
        const price = product.price.final;
        if (!link || !image || !price || !Number.isFinite(Number(price))) throw new Error(`Incomplete product ${id}`);
        const imageUrl = decodeURIComponent(image.getAttribute('data-src') || image.src).match(/https:\/\/static\.theiconic\.com\.au\/p\/[^?]+/)?.[0] || '';
        if (!imageUrl.startsWith('https://static.theiconic.com.au/p/')) throw new Error('Unexpected image host');
        found.push({product_id:id,title:product.name,brand:product.brand,price:`A$${Number(price).toFixed(2)} AUD`,product_url:link.href,image_url:imageUrl});
      }
      const next = [...document.querySelectorAll('a[href]')].some(a => { const u = new URL(a.href); return u.pathname === location.pathname && Number(u.searchParams.get('page')) === Number(new URL(location.href).searchParams.get('page')) + 1; });
      return { products: found, next };
    });
    for (const product of result.products) products.set(product.product_id, product);
    if (!result.next) return { data: { products: [...products.values()], pages: number }, type: 'application/json' };
  }
  throw new Error('Pagination exceeded 20 pages');
}
