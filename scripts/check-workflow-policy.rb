# frozen_string_literal: true

require 'psych'

class WorkflowPolicy
  class Invalid < StandardError; end

  def initialize(path)
    @path = path
    @failed = false
  end

  def check
    # Inspect YAML nodes, not deserialized Ruby objects. This preserves key
    # spelling, detects duplicate keys, and never constructs tagged objects.
    documents = Psych.parse_stream(File.binread(@path)).children
    raise Invalid, 'expected one YAML document' unless documents.size == 1

    workflow = mapping(documents.first.root, 'workflow')
    check_scripts(workflow)
    read_only_permissions = check_permissions(workflow['permissions'])
    !@failed && read_only_permissions
  rescue Invalid => e
    warn "  FAIL: #{@path}: #{e.message}"
    false
  rescue Psych::SyntaxError => e
    warn "  FAIL: #{@path}:#{e.line}: invalid YAML"
    false
  end

  private

  def mapping(node, context)
    unless node.is_a?(Psych::Nodes::Mapping) && node.tag.nil?
      raise Invalid, "expected a YAML mapping for #{context}"
    end

    node.children.each_slice(2).each_with_object({}) do |(key, value), entries|
      unless scalar?(key) && key.value != '<<'
        raise Invalid, "unsupported YAML key or merge in #{context}"
      end

      name = key.value.downcase
      raise Invalid, "duplicate key in #{context} at line #{key.start_line + 1}" if entries.key?(name)

      entries[name] = value
    end
  end

  def scalar?(node)
    node.is_a?(Psych::Nodes::Scalar) && node.tag.nil?
  end

  def check_permissions(node)
    read_only = if scalar?(node)
                  node.value == 'read-all'
                elsif node.is_a?(Psych::Nodes::Mapping)
                  mapping(node, 'top-level permissions').values.all? do |value|
                    scalar?(value) && %w[read none].include?(value.value)
                  end
                end
    if read_only
      puts '  PASS: top-level permissions are explicitly read-only'
      true
    else
      warn '  FAIL: top-level permissions must be explicitly read-only (grant write per job)'
      false
    end
  end

  def check_scripts(workflow)
    mapping(workflow['jobs'], 'jobs').each_value do |job|
      steps = mapping(job, 'job')['steps']
      next unless steps # Jobs calling reusable workflows have no steps.

      unless steps.is_a?(Psych::Nodes::Sequence) && steps.tag.nil?
        raise Invalid, 'expected a YAML sequence for steps'
      end

      steps.children.each do |step|
        run = mapping(step, 'step')['run']
        next unless run
        raise Invalid, 'expected a YAML scalar for run' unless scalar?(run)
        next unless run.value.include?('${{')

        warn "  FAIL: #{@path}:#{run.start_line + 1}: an expression is pasted into a run: script (pass it through env)"
        @failed = true
      end
    end

    puts '  PASS: no expressions pasted into run: scripts' unless @failed
  end
end

exit(WorkflowPolicy.new(ARGV.fetch(0)).check ? 0 : 1)
