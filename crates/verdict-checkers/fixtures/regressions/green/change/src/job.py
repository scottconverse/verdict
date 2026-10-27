import logging


def run(job):
    try:
        return job()
    except ValueError as err:
        logging.warning("job failed: %s", err)
        return None
