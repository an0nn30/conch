.PHONY: build clean

SDK_JAR = ../../java-sdk/build/termlab-plugin-sdk.jar
BUILD_DIR = build
CLASSES_DIR = $(BUILD_DIR)/classes
JAR = $(BUILD_DIR)/hello-plugin.jar

build: $(SDK_JAR)
	mkdir -p $(CLASSES_DIR)
	javac -cp $(SDK_JAR) -d $(CLASSES_DIR) src/termlab/plugin/hello/*.java
	jar cfm $(JAR) MANIFEST.MF -C $(CLASSES_DIR) .
	@echo "Built: $(JAR)"

$(SDK_JAR):
	$(MAKE) -C ../../java-sdk build

clean:
	rm -rf $(BUILD_DIR)
