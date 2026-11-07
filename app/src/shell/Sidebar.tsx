import { ListMusic, Settings } from 'lucide-react';
import { PrefetchLink, PrefetchNavLink } from '@/components/PrefetchLink';
import {
  AlbumsIcon,
  AllPlaylistsIcon,
  ArtistsIcon,
  HomeIcon,
  MadeForYouIcon,
  NewIcon,
  RecentlyAddedIcon,
  SearchIcon,
  SongsIcon,
  type NavIconProps,
} from '@/components/icons/NavIcons';
import { hrefFor } from '@/components/music/links';
import { LIBRARY_SECTIONS, type LibrarySection } from '@/data/music/library';
import { useLibraryPlaylists, useSignedIn } from '@/data/music/queries';
import { useSpotifyPlaylists } from '@/data/spotify/queries';
import { useSpotifySignedIn } from '@/extension/useSpotifyAuth';
import { useUi } from '@/state/ui';
import { AccountButton } from './AccountButton';

interface NavItem {
  to: string;
  label: string;
  Icon: (props: NavIconProps) => React.ReactNode;
}

const PRIMARY: NavItem[] = [
  { to: '/search', label: 'Search', Icon: SearchIcon },
  { to: '/home', label: 'Home', Icon: HomeIcon },
  { to: '/new', label: 'New', Icon: NewIcon },
];

const LIBRARY_ICONS: Record<Exclude<LibrarySection, 'playlists'>, NavItem['Icon']> = {
  recent: RecentlyAddedIcon,
  artists: ArtistsIcon,
  albums: AlbumsIcon,
  songs: SongsIcon,
  'made-for-you': MadeForYouIcon,
};

const LIBRARY: NavItem[] = (Object.keys(LIBRARY_ICONS) as (keyof typeof LIBRARY_ICONS)[]).map((section) => ({
  to: `/library/${section}`,
  label: LIBRARY_SECTIONS[section].title,
  Icon: LIBRARY_ICONS[section],
}));

// A sidebar row; NavLink sets aria-current on the page's own row.
const item =
  'group flex h-9 items-center gap-2 mb-0.5 p-1 rounded-nav type-body-reduced text-primary hover:bg-quinary aria-[current=page]:bg-nav-selected';
// Apple's glyphs, drawn at 24px (their 24-unit box), which the user preferred to Apple's own 28px.
const icon = 'inline-grid size-6 flex-none place-items-center text-primary group-aria-[current=page]:text-key';
const navLabel = 'min-w-0 truncate';
// "Library", "Playlists" (report §1: 12px/15px 600, secondary, padding 4px 8px).
const sectionHeader = 'mt-4 px-2 py-1 type-callout-emphasized text-secondary';
const list = 'flex flex-col';

function NavList({ items, onNavigate }: { items: NavItem[]; onNavigate: () => void }) {
  return (
    <ul className={list}>
      {items.map(({ to, label, Icon }) => (
        <li key={to}>
          <PrefetchNavLink to={to} className={item} onClick={onNavigate}>
            <Icon size={24} className={icon} />
            <span className={navLabel}>{label}</span>
          </PrefetchNavLink>
        </li>
      ))}
    </ul>
  );
}

/** Library and Playlists (report §1), once YouTube requests are made as the user. */
function LibraryNav({ onNavigate }: { onNavigate: () => void }) {
  const playlists = useLibraryPlaylists();
  return (
    <>
      <h2 className={sectionHeader}>Library</h2>
      <NavList items={LIBRARY} onNavigate={onNavigate} />
      <h2 className={sectionHeader}>Playlists</h2>
      <ul className={list}>
        <li>
          <PrefetchNavLink to="/library/playlists" className={item} onClick={onNavigate}>
            <AllPlaylistsIcon size={24} className={icon} />
            <span className={navLabel}>{LIBRARY_SECTIONS.playlists.title}</span>
          </PrefetchNavLink>
        </li>
        {playlists.data?.map((playlist) => (
          <li key={playlist.id}>
            <PrefetchNavLink to={hrefFor(playlist) ?? '/library/playlists'} className={item} onClick={onNavigate}>
              <span className={icon}>
                <ListMusic size={17} strokeWidth={1.8} />
              </span>
              <span className={navLabel}>{playlist.title}</span>
            </PrefetchNavLink>
          </li>
        ))}
      </ul>
    </>
  );
}

/** Spotify's playlists, once the extension can read them as the user (prompt §7.1). */
function SpotifyNav({ onNavigate }: { onNavigate: () => void }) {
  const playlists = useSpotifyPlaylists();
  return (
    <>
      <h2 className={sectionHeader}>Spotify</h2>
      <ul className={list}>
        <li>
          <PrefetchNavLink to="/spotify" end className={item} onClick={onNavigate}>
            <AllPlaylistsIcon size={24} className={icon} />
            <span className={navLabel}>All Spotify Playlists</span>
          </PrefetchNavLink>
        </li>
        {playlists.data?.map((playlist) => (
          <li key={playlist.id}>
            <PrefetchNavLink to={`/spotify/playlist/${encodeURIComponent(playlist.id)}`} className={item} onClick={onNavigate}>
              <span className={icon}>
                <ListMusic size={17} strokeWidth={1.8} />
              </span>
              <span className={navLabel}>{playlist.name}</span>
            </PrefetchNavLink>
          </li>
        ))}
      </ul>
    </>
  );
}

/** The left glass sidebar (report §1). */
export function Sidebar() {
  const drawerOpen = useUi((s) => s.drawerOpen);
  const setDrawerOpen = useUi((s) => s.setDrawerOpen);
  const signedIn = useSignedIn();
  const spotifySignedIn = useSpotifySignedIn();
  const close = () => setDrawerOpen(false);

  return (
    <>
      {/* Below 484px the sidebar is a drawer opened from the top bar's menu button. */}
      <nav
        className="absolute inset-y-0 left-0 z-(--z-web-chrome) flex w-(--web-navigation-width) flex-col glass-material max-sm:w-[min(300px,82vw)] max-sm:-translate-x-full max-sm:shadow-glass max-sm:transition-transform max-sm:duration-(--duration-panel) max-sm:ease-panel max-sm:data-[open=true]:translate-none"
        data-open={drawerOpen}
        aria-label="Main"
      >
        {/* The height the wordmark used to occupy: it keeps the macOS traffic
            lights their air and the window's drag zone its room. The brand
            itself moved out of the window's top — the frameless top bar is
            the app's own page now, not a place for a logo. */}
        <div className="mt-2 ml-2 h-18 shrink-0" aria-hidden="true" />
        <div className="min-h-0 flex-1 overflow-y-auto pr-3 pl-5 [scrollbar-width:none]">
          <NavList items={PRIMARY} onNavigate={close} />
          {signedIn && <LibraryNav onNavigate={close} />}
          {spotifySignedIn && <SpotifyNav onNavigate={close} />}
        </div>
        <div className="flex items-center justify-between pt-3 pr-3 pb-5 pl-5">
          <AccountButton />
          <PrefetchNavLink to="/settings" className="grid size-7 place-items-center rounded-nav text-secondary hover:bg-quinary hover:text-primary aria-[current=page]:bg-quinary aria-[current=page]:text-primary" aria-label="Settings" title="Settings" onClick={close}>
            <Settings size={17} strokeWidth={1.9} />
          </PrefetchNavLink>
        </div>
      </nav>
      {drawerOpen && <div className="absolute inset-0 z-[calc(var(--z-web-chrome)-1)] hidden bg-scrim max-sm:block" onClick={close} aria-hidden="true" />}
    </>
  );
}
