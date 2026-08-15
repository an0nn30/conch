.PHONY: build clean

BUILD_DIR = build
CLASSES_DIR = $(BUILD_DIR)/classes
JAVADOC_DIR = $(BUILD_DIR)/javadoc
SOURCES = $(wildcard src/termlab/plugin/*.java)

JAR          = $(BUILD_DIR)/termlab-plugin-sdk.jar
SOURCES_JAR  = $(BUILD_DIR)/termlab-plugin-sdk-sources.jar
JAVADOC_JAR  = $(BUILD_DIR)/termlab-plugin-sdk-javadoc.jar

build: $(JAR) $(SOURCES_JAR) $(JAVADOC_JAR)

$(JAR): $(SOURCES)
	mkdir -p $(CLASSES_DIR)
	javac -d $(CLASSES_DIR) $(SOURCES)
	jar cf $(JAR) -C $(CLASSES_DIR) .
	@echo "Built: $(JAR)"

$(SOURCES_JAR): $(SOURCES)
	jar cf $(SOURCES_JAR) -C src .
	@echo "Built: $(SOURCES_JAR)"

$(JAVADOC_JAR): $(SOURCES)
	mkdir -p $(JAVADOC_DIR)
	javadoc -d $(JAVADOC_DIR) -quiet -sourcepath src termlab.plugin
	jar cf $(JAVADOC_JAR) -C $(JAVADOC_DIR) .
	@echo "Built: $(JAVADOC_JAR)"

clean:
	rm -rf $(BUILD_DIR)
