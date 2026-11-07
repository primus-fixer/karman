import type { Snapshot } from '../api';
import type { Nav } from '../App';
import { Figures, TripList } from './Overview';

export default function Trips({ snap, nav }: { snap: Snapshot; nav: Nav }) {
  const open = snap.trips.filter((t) => t.trip.status !== 'closed');
  const closed = snap.trips.filter((t) => t.trip.status === 'closed');
  return (
    <div className="stack gap20">
      <h1 className="h1">Поездки</h1>
      <Figures snap={snap} nav={nav} big={false} />
      <section className="stack">
        <h2 className="h2" style={{ marginBottom: 8 }}>
          Открытые
        </h2>
        {open.length === 0 ? <p className="empty">Открытых поездок нет.</p> : <TripList snap={snap} nav={nav} />}
      </section>
      {closed.length > 0 && (
        <section className="stack">
          <h2 className="h2" style={{ marginBottom: 8 }}>
            Закрытые
          </h2>
          <TripList snap={snap} nav={nav} closed />
        </section>
      )}
      <button type="button" className="btn ghost wide" onClick={() => nav.newTrip()}>
        Новая поездка
      </button>
    </div>
  );
}
