def run(job):
    try:
        return job()
    except:
        return None
