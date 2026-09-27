import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

export default defineConfig({
  site: 'https://nyabia.github.io',
  base: '/nyatidraw/docs',
  trailingSlash: 'always',
  outDir: '../target/site-docs',
  integrations: [
    starlight({
      title: 'NyatiDraw 문서',
      description: 'NyatiDraw 사용법과 웹 연동 안내',
      logo: { src: '../site/assets/nyatidraw.svg', alt: '' },
      favicon: 'https://nyabia.github.io/nyatidraw/assets/nyatidraw.svg',
      locales: { root: { label: '한국어', lang: 'ko' } },
      lastUpdated: false,
      customCss: ['./src/styles/docs.css'],
      social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/nyabia/nyatidraw' }],
      sidebar: [
        { label: '홈페이지', link: 'https://nyabia.github.io/nyatidraw/' },
        { label: '웹에서 그리기 ↗', link: 'https://nyabia.github.io/nyatidraw/draw/' },
        { label: '문서 안내', slug: 'index' },
        { label: '사용하기', items: [{ autogenerate: { directory: 'guide' } }] },
        { label: '연동하기', items: [{ autogenerate: { directory: 'integration' } }] },
      ],
    }),
  ],
});
