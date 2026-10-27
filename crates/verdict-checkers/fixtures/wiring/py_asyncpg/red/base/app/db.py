import asyncpg

pool: asyncpg.Pool | None = None


async def connect(dsn: str) -> None:
    global pool
    pool = await asyncpg.create_pool(dsn)
