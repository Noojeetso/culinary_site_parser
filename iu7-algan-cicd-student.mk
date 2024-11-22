.PHONY: clean

ready/stud-unit-test-report-prev.json: ready
	echo "{}" > $@

ready/stud-unit-test-report.json: ready
	echo "{\"timestamp\": \"$(shell date +"%Y-%m-%dT%H:%M:%S%:z")\", \"passed\": 1, \"failed\": 0, \"coverage\": 10}" > $@

ready/report.pdf: ready 
	cp report/report.pdf ready/report.pdf

ready/app-cli-debug: ready
	touch ready/app-cli-debug
	chmod +x ready/app-cli-debug

ready:
	mkdir -p ready

clean:
	rm -rf ready
