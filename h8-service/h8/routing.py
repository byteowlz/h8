"""Routing providers for trip planning.

Supports:
- OSRM (Open Source Routing Machine) for car routing (free, no API key, global)
- Nominatim (OpenStreetMap) for geocoding (free, no API key, global)
- Deutsche Bahn HAFAS for train routing in Germany (via public API)
- Extensible for other transit providers (SBB, SNCF, Amtrak, etc.)
"""

import logging
import math
from dataclasses import dataclass, field
from datetime import datetime, timedelta
from typing import Optional

import httpx

logger = logging.getLogger(__name__)

# --- Geocoding (Nominatim / OSM, worldwide) ---

NOMINATIM_URL = "https://nominatim.openstreetmap.org/search"
NOMINATIM_HEADERS = {"User-Agent": "h8-trip-planner/1.0"}


@dataclass
class GeoLocation:
    """A geocoded location with coordinates."""

    lat: float
    lon: float
    display_name: str
    address: str


async def geocode(
    query: str, country: Optional[str] = None
) -> Optional[GeoLocation]:
    """Geocode an address or place name to coordinates using Nominatim (global).

    Args:
        query: Address, city, or place name.
        country: Optional ISO 3166-1 alpha-2 country code to bias results
                 (e.g., "de", "us", "ch"). None searches worldwide.
    """
    params: dict = {
        "q": query,
        "format": "json",
        "limit": 1,
        "addressdetails": 1,
    }
    if country:
        params["countrycodes"] = country

    async with httpx.AsyncClient(timeout=10) as client:
        resp = await client.get(
            NOMINATIM_URL, params=params, headers=NOMINATIM_HEADERS
        )
        resp.raise_for_status()
        results = resp.json()

    if not results:
        return None

    r = results[0]
    return GeoLocation(
        lat=float(r["lat"]),
        lon=float(r["lon"]),
        display_name=r.get("display_name", query),
        address=query,
    )


# --- Car Routing (OSRM) ---

OSRM_URL = "https://router.project-osrm.org/route/v1/driving"


@dataclass
class CarRoute:
    """Car routing result."""

    duration_seconds: float
    distance_meters: float
    duration_minutes: int
    distance_km: float


async def route_car_osrm(
    origin_lon: float,
    origin_lat: float,
    dest_lon: float,
    dest_lat: float,
) -> Optional[CarRoute]:
    """Get driving route from OSRM (public, free, no API key)."""
    url = f"{OSRM_URL}/{origin_lon},{origin_lat};{dest_lon},{dest_lat}"
    params = {"overview": "false"}

    async with httpx.AsyncClient(timeout=15) as client:
        resp = await client.get(url, params=params)
        resp.raise_for_status()
        data = resp.json()

    if data.get("code") != "Ok" or not data.get("routes"):
        return None

    route = data["routes"][0]
    duration = route["duration"]
    distance = route["distance"]

    return CarRoute(
        duration_seconds=duration,
        distance_meters=distance,
        duration_minutes=math.ceil(duration / 60),
        distance_km=round(distance / 1000, 1),
    )


# --- Public Transit Routing ---

# Transit providers: pluggable backends for different countries/networks.
# Each provider implements station search + journey search.


@dataclass
class TransitLeg:
    """A single leg of a public transit journey."""

    line: str  # e.g. "ICE 945", "S3", "Bus 42", or "" for walking
    departure_station: str
    arrival_station: str
    departure_time: str  # ISO format
    arrival_time: str  # ISO format
    duration_minutes: int
    platform: Optional[str] = None
    arrival_platform: Optional[str] = None
    mode: Optional[str] = None  # "train", "bus", "subway", "walking", etc.
    walking: bool = False
    distance_meters: Optional[int] = None  # for walking legs


@dataclass
class TransitJourney:
    """A complete transit journey with one or more legs."""

    legs: list[TransitLeg]
    total_duration_minutes: int
    departure_time: str
    arrival_time: str
    changes: int
    provider: str  # Which provider found this journey


# --- Deutsche Bahn HAFAS provider ---

DB_HAFAS_URL = "https://v6.db.transport.rest"


async def _db_search_station(query: str, retries: int = 2) -> Optional[dict]:
    """Search for a train station by name using DB HAFAS.

    Retries on 5xx errors since the public API is intermittently unavailable.
    """
    import asyncio

    params = {"query": query, "results": 3, "stops": "true", "addresses": "false"}
    max_attempts = retries + 1
    for attempt in range(max_attempts):
        resp = None
        try:
            async with httpx.AsyncClient(timeout=20) as client:
                resp = await client.get(f"{DB_HAFAS_URL}/locations", params=params)
        except (httpx.TimeoutException, httpx.HTTPError) as e:
            if attempt < max_attempts - 1:
                wait = 2 ** attempt
                logger.warning(
                    "DB HAFAS station search error for '%s': %s, retrying in %ds (attempt %d/%d)...",
                    query, type(e).__name__, wait, attempt + 1, max_attempts,
                )
                await asyncio.sleep(wait)
                continue
            logger.warning(
                "DB HAFAS station search failed for '%s' after %d attempts: %s",
                query, max_attempts, e,
            )
            return None

        if resp.status_code >= 500:
            if attempt < max_attempts - 1:
                wait = 2 ** attempt
                logger.warning(
                    "DB HAFAS returned %d for '%s', retrying in %ds (attempt %d/%d)...",
                    resp.status_code, query, wait, attempt + 1, max_attempts,
                )
                await asyncio.sleep(wait)
                continue
            logger.warning(
                "DB HAFAS returned %d for '%s' after %d attempts",
                resp.status_code, query, max_attempts,
            )
            return None

        if resp.status_code >= 400:
            logger.warning("DB HAFAS station search returned %d for '%s'", resp.status_code, query)
            return None

        results = resp.json()
        for r in results:
            if r.get("type") == "station":
                return r
        if results:
            return results[0]
        return None

    return None


async def _db_route_transit(
    origin_station: str,
    dest_station: str,
    departure: Optional[datetime] = None,
    arrival: Optional[datetime] = None,
    results: int = 3,
    origin_lat: Optional[float] = None,
    origin_lon: Optional[float] = None,
    dest_lat: Optional[float] = None,
    dest_lon: Optional[float] = None,
) -> list[TransitJourney]:
    """Search for connections via Deutsche Bahn HAFAS.

    When coordinates are provided alongside station names, HAFAS routes
    door-to-door including walking/bus legs for the last mile.

    Args:
        origin_station: Origin station name or address label.
        dest_station: Destination station name or address label.
        departure: Find connections departing at/after this time.
        arrival: Find connections arriving at/before this time.
        origin_lat/lon: Origin coordinates (enables door-to-door routing).
        dest_lat/lon: Destination coordinates (enables door-to-door routing).
    """
    # Build origin parameter: prefer coordinates (door-to-door), fall back to station ID
    from_params: dict = {}
    if origin_lat is not None and origin_lon is not None:
        from_params = {
            "from.latitude": origin_lat,
            "from.longitude": origin_lon,
            "from.address": origin_station,
        }
    else:
        origin = await _db_search_station(origin_station)
        if not origin or not origin.get("id"):
            logger.warning("Could not resolve origin station: %s", origin_station)
            return []
        from_params = {"from": origin["id"]}

    # Build destination parameter
    to_params: dict = {}
    if dest_lat is not None and dest_lon is not None:
        to_params = {
            "to.latitude": dest_lat,
            "to.longitude": dest_lon,
            "to.address": dest_station,
        }
    else:
        dest = await _db_search_station(dest_station)
        if not dest or not dest.get("id"):
            logger.warning("Could not resolve destination station: %s", dest_station)
            return []
        to_params = {"to": dest["id"]}

    params: dict = {**from_params, **to_params, "results": results}
    if arrival:
        params["arrival"] = arrival.isoformat()
    elif departure:
        params["departure"] = departure.isoformat()

    import asyncio

    data = None
    max_attempts = 3
    for attempt in range(max_attempts):
        try:
            async with httpx.AsyncClient(timeout=30) as client:
                resp = await client.get(f"{DB_HAFAS_URL}/journeys", params=params)
        except httpx.TimeoutException:
            if attempt < max_attempts - 1:
                logger.warning(
                    "DB HAFAS journey search timed out, retrying (attempt %d/%d)...",
                    attempt + 1, max_attempts,
                )
                await asyncio.sleep(2 ** attempt)
                continue
            logger.warning(
                "DB HAFAS journey search timed out: %s -> %s (after %d attempts)",
                origin_station, dest_station, max_attempts,
            )
            return []
        except httpx.HTTPError as e:
            logger.warning(
                "DB HAFAS journey search failed (%s -> %s): %s",
                origin_station, dest_station, e,
            )
            return []

        if resp.status_code >= 500:
            if attempt < max_attempts - 1:
                logger.warning(
                    "DB HAFAS returned %d, retrying in %ds (attempt %d/%d)...",
                    resp.status_code, 2 ** attempt, attempt + 1, max_attempts,
                )
                await asyncio.sleep(2 ** attempt)
                continue
            logger.warning(
                "DB HAFAS returned %d after %d attempts", resp.status_code, max_attempts
            )
            return []

        try:
            resp.raise_for_status()
        except httpx.HTTPError as e:
            logger.warning("DB HAFAS journey search error: %s", e)
            return []

        data = resp.json()
        break

    if data is None:
        return []

    return _parse_hafas_journeys(data, provider="db")


def _parse_hafas_journeys(data: dict, provider: str) -> list[TransitJourney]:
    """Parse HAFAS-format journey responses (shared by DB, SBB, etc.).

    Includes walking legs, platform changes, and last-mile routing.
    """
    journeys = []
    for j in data.get("journeys", []):
        legs = []
        for leg in j.get("legs", []):
            is_walking = bool(leg.get("walking"))
            dep_station = leg.get("origin", {}).get("name", "?")
            arr_station = leg.get("destination", {}).get("name", "?")
            dep_time = leg.get("departure", "")
            arr_time = leg.get("arrival", "")

            dur_min = 0
            if dep_time and arr_time:
                try:
                    dt_dep = datetime.fromisoformat(dep_time.replace("Z", "+00:00"))
                    dt_arr = datetime.fromisoformat(arr_time.replace("Z", "+00:00"))
                    dur_min = int((dt_arr - dt_dep).total_seconds() / 60)
                except (ValueError, TypeError):
                    pass

            if is_walking:
                # Skip zero-duration platform transfers (same station)
                distance = leg.get("distance")
                if dur_min == 0 and (distance is None or distance < 50):
                    continue
                legs.append(
                    TransitLeg(
                        line="",
                        departure_station=dep_station,
                        arrival_station=arr_station,
                        departure_time=dep_time,
                        arrival_time=arr_time,
                        duration_minutes=dur_min,
                        mode="walking",
                        walking=True,
                        distance_meters=distance,
                    )
                )
            else:
                line = leg.get("line", {})
                line_name = line.get("name", "")
                platform = leg.get("departurePlatform")
                arr_platform = leg.get("arrivalPlatform")
                line_mode = line.get("mode")

                legs.append(
                    TransitLeg(
                        line=line_name,
                        departure_station=dep_station,
                        arrival_station=arr_station,
                        departure_time=dep_time,
                        arrival_time=arr_time,
                        duration_minutes=dur_min,
                        platform=platform,
                        arrival_platform=arr_platform,
                        mode=line_mode,
                    )
                )

        if not legs:
            continue

        total_dep = legs[0].departure_time
        total_arr = legs[-1].arrival_time
        total_dur = 0
        if total_dep and total_arr:
            try:
                dt_dep = datetime.fromisoformat(total_dep.replace("Z", "+00:00"))
                dt_arr = datetime.fromisoformat(total_arr.replace("Z", "+00:00"))
                total_dur = int((dt_arr - dt_dep).total_seconds() / 60)
            except (ValueError, TypeError):
                pass

        # Count changes: number of non-walking legs minus 1
        transport_legs = [l for l in legs if not l.walking]
        num_changes = max(0, len(transport_legs) - 1)

        journeys.append(
            TransitJourney(
                legs=legs,
                total_duration_minutes=total_dur,
                departure_time=total_dep,
                arrival_time=total_arr,
                changes=num_changes,
                provider=provider,
            )
        )

    return journeys


# --- Transit provider registry ---

# Map of provider name -> route function.
# Add new providers here (SBB, SNCF, etc.).
TRANSIT_PROVIDERS: dict[str, type] = {}


async def route_transit(
    origin_station: str,
    dest_station: str,
    provider: str = "db",
    departure: Optional[datetime] = None,
    arrival: Optional[datetime] = None,
    results: int = 3,
    origin_lat: Optional[float] = None,
    origin_lon: Optional[float] = None,
    dest_lat: Optional[float] = None,
    dest_lon: Optional[float] = None,
) -> list[TransitJourney]:
    """Route via public transit using the specified provider.

    When coordinates are provided, enables door-to-door routing including
    walking/bus legs for the last mile.
    """
    if provider == "db":
        return await _db_route_transit(
            origin_station, dest_station, departure, arrival, results,
            origin_lat, origin_lon, dest_lat, dest_lon,
        )
    else:
        logger.error("Unknown transit provider: %s (available: db)", provider)
        return []


# --- Unified routing interface ---


@dataclass
class RouteResult:
    """Unified route result for any transport mode."""

    mode: str  # "car" or "transit"
    duration_minutes: int
    distance_km: Optional[float]
    # Car-specific
    car_route: Optional[CarRoute] = None
    # Transit-specific
    transit_journeys: Optional[list[TransitJourney]] = None


async def calculate_route(
    origin_lat: float,
    origin_lon: float,
    dest_lat: float,
    dest_lon: float,
    mode: str = "car",
    origin_station: Optional[str] = None,
    dest_station: Optional[str] = None,
    transit_provider: str = "db",
    departure: Optional[datetime] = None,
    arrival: Optional[datetime] = None,
) -> Optional[RouteResult]:
    """Calculate a route using the appropriate provider.

    Args:
        origin_lat, origin_lon: Origin coordinates (global).
        dest_lat, dest_lon: Destination coordinates (global).
        mode: "car" or "transit".
        origin_station: Station name for transit routing.
        dest_station: Station name for transit routing.
        transit_provider: Transit provider name (e.g., "db", "sbb").
        departure: Desired departure time (for transit timetables).
        arrival: Desired arrival time (find connections arriving by this time).

    Returns:
        RouteResult or None if routing failed.
    """
    if mode == "car":
        route = await route_car_osrm(origin_lon, origin_lat, dest_lon, dest_lat)
        if not route:
            return None
        return RouteResult(
            mode="car",
            duration_minutes=route.duration_minutes,
            distance_km=route.distance_km,
            car_route=route,
        )
    elif mode == "transit":
        if not origin_station or not dest_station:
            logger.error("Transit routing requires station names")
            return None
        journeys = await route_transit(
            origin_station, dest_station, transit_provider, departure, arrival,
            origin_lat=origin_lat, origin_lon=origin_lon,
            dest_lat=dest_lat, dest_lon=dest_lon,
        )
        if not journeys:
            return None
        first = journeys[0]
        return RouteResult(
            mode="transit",
            duration_minutes=first.total_duration_minutes,
            distance_km=None,
            transit_journeys=journeys,
        )
    else:
        logger.error("Unknown routing mode: %s (available: car, transit)", mode)
        return None
